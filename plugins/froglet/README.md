# Froglet Agent Plugin

This directory packages Froglet for agent hosts that support plugin bundles.
It intentionally does not duplicate the MCP implementation. The plugin starts
`npx -y froglet-mcp` and lets the published MCP package own runtime behavior.

## Included surfaces

- Codex manifest: `.codex-plugin/plugin.json`
- Claude Code manifest: `.claude-plugin/plugin.json`
- Shared MCP config: `.mcp.json`
- Shared agent guidance skill: `skills/froglet/SKILL.md`
- Claude slash command: `commands/froglet.md`

## Default behavior

The plugin starts `npx -y froglet-mcp` in `FROGLET_PROFILE=local` against the
default local provider/runtime ports:

- provider: `http://127.0.0.1:8080`
- runtime: `http://127.0.0.1:8081`
- provider token: `data/runtime/froglet-control.token`
- runtime token: `data/runtime/auth.token`

The first action should be `status`. If the local node is not running, use
`plan_install` and `get_install_guide` to guide the user through Docker,
binary, or source setup. Once local health is verified, use `plan_use_case`
before implementing consumer, provider, evidence, payments, batch, or GPU
workflows. The public no-install demo remains the HTTP
`llms.txt` flow at `https://froglet.dev/llms.txt`; it is not exposed as an
installed plugin action.

In Claude Code, `/froglet` follows the same rule: status first, install planning
second, and real local actions only after the provider/runtime are reachable.
