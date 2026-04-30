---
allowed-tools: mcp__plugin_froglet_froglet__froglet
description: Check local Froglet status, plan install, or stage a real use case
disable-model-invocation: false
---

Use the installed Froglet MCP tool as the source of truth. Do not use the
hosted demo as a substitute for local/plugin behavior.

User request:

```
$ARGUMENTS
```

Flow:

1. Call the Froglet MCP tool with `{"action":"status"}` first.
2. If local provider/runtime URLs or token paths are missing, explain the exact
   local gap and do not claim Froglet is installed.
3. If the user asks to install or the local node is not ready, call
   `plan_install` before suggesting shell commands. Ask for missing choices:
   target agent, install footprint, node role, network mode, payment rail,
   marketplace URL, and first use case.
4. If the install profile is complete, call `get_install_guide` and tell the
   user that shell commands run through the host agent shell, not through the
   Froglet runtime.
5. If Froglet is already reachable, call `plan_use_case` before implementing
   the user's first workflow, especially for batch or GPU requests.
6. Use the MCP actions for real local work:
   service discovery, invocation, artifacts, settlement inspection, marketplace
   operations, or the requested use-case implementation.

Keep the answer evidence-first: report tool status, exact errors, and what is
or is not proven for the user's local setup.
