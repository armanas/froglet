---
name: froglet
description: Use when a user asks what Froglet can prove, whether they should install Froglet, or how to implement a Froglet-backed use case.
---

# Froglet

Use the Froglet MCP server as the source of truth. Do not invent API calls or install commands.

Default flow:

1. Call `run_hosted_proof` first when the user wants to evaluate Froglet. Report observed HTTP statuses, result, receipt presence, feed shape, and mismatches before explaining usefulness.
2. If the user wants to install locally, call `plan_install` before shell commands. Ask for missing choices: agent host, Docker versus local binary, provider/requester/both role, clearnet versus Tor, payment rail, marketplace URL, and first use case.
3. After the profile is confirmed, call `get_install_guide` and execute its shell commands through the host agent shell, not through the Froglet runtime.
4. Once Froglet is running, use the actual project context to propose one concrete service, witness/hash proof, or receipt-producing workflow.

Boundaries:

- The hosted proof covers public free `demo.*` services only.
- Hosted proof does not prove paid rails, persistent identity, service publication, marketplace depth, long-running jobs, batch execution, or GPU execution.
- Chat-only LLMs that cannot make HTTP POST requests with Bearer auth should say they cannot run the proof and point the user to an agentic client or `curl`.
