# Froglet Agent Plugin

This directory packages Froglet for agent hosts that support plugin bundles.
It intentionally does not duplicate the MCP implementation. The plugin starts
`npx -y froglet-mcp` and lets the published MCP package own runtime behavior.

## Included surfaces

- Codex manifest: `.codex-plugin/plugin.json`
- Claude Code manifest: `.claude-plugin/plugin.json`
- Shared MCP config: `.mcp.json`
- Shared agent guidance skill: `skills/froglet/SKILL.md`

## Default behavior

The MCP server starts with `FROGLET_PROFILE=hosted-proof`, so the first action
should be `run_hosted_proof`. Local install and payment-rail setup are staged
through `plan_install` and `get_install_guide` after the user confirms the
profile.
