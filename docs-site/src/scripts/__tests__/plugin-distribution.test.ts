import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '../../../..');

function readRepoFile(path: string): string {
  return readFileSync(resolve(repoRoot, path), 'utf8');
}

function readJson(path: string): any {
  return JSON.parse(readRepoFile(path));
}

describe('plugin and registry distribution metadata', () => {
  it('keeps MCP Registry metadata aligned with npm package metadata', () => {
    const pkg = readJson('package.json');
    const server = readJson('server.json');

    expect(pkg.name).toBe('froglet-mcp');
    expect(pkg.mcpName).toBe('io.github.armanas/froglet');
    expect(pkg.license).toBe('Apache-2.0');
    expect(pkg.files).toContain('server.json');
    expect(server.name).toBe(pkg.mcpName);
    expect(server.version).toBe(pkg.version);
    expect(server.packages[0].identifier).toBe(pkg.name);
    expect(server.packages[0].version).toBe(pkg.version);
    expect(server.packages[0].transport.type).toBe('stdio');
  });

  it('packages Codex and Claude plugins around the same hosted-proof MCP command', () => {
    const codex = readJson('plugins/froglet/.codex-plugin/plugin.json');
    const claude = readJson('plugins/froglet/.claude-plugin/plugin.json');
    const mcp = readJson('plugins/froglet/.mcp.json');

    expect(codex.name).toBe('froglet');
    expect(claude.name).toBe('froglet');
    expect(codex.license).toBe('Apache-2.0');
    expect(claude.license).toBe('Apache-2.0');
    expect(codex.mcpServers).toBe('./.mcp.json');
    expect(claude.mcpServers).toBe('./.mcp.json');
    expect(mcp.mcpServers.froglet.command).toBe('npx');
    expect(mcp.mcpServers.froglet.args).toEqual(['-y', 'froglet-mcp']);
    expect(mcp.mcpServers.froglet.env.FROGLET_PROFILE).toBe('hosted-proof');
  });

  it('publishes repo-local marketplace entries for Codex and Claude Code', () => {
    const codexMarketplace = readJson('.agents/plugins/marketplace.json');
    const claudeMarketplace = readJson('.claude-plugin/marketplace.json');

    expect(codexMarketplace.name).toBe('froglet');
    expect(codexMarketplace.plugins[0].source.path).toBe('./plugins/froglet');
    expect(codexMarketplace.plugins[0].policy.installation).toBe('AVAILABLE');
    expect(codexMarketplace.plugins[0].policy.authentication).toBe('ON_INSTALL');
    expect(claudeMarketplace.name).toBe('froglet');
    expect(claudeMarketplace.plugins[0].source).toBe('./plugins/froglet');
    expect(claudeMarketplace.plugins[0].version).toBe(readJson('package.json').version);
  });

  it('documents distribution order and hosted-proof boundaries on website and repo docs', () => {
    for (const path of [
      'docs/PLUGIN_DISTRIBUTION.md',
      'docs-site/src/content/docs/learn/plugin-distribution.mdx',
    ]) {
      const text = readRepoFile(path);
      expect(text).toContain('Official MCP Registry');
      expect(text).toContain('ChatGPT App Directory');
      expect(text).toContain('hosted public MCP');
      expect(text).toContain('https://apps.froglet.dev/mcp');
      expect(text).toContain('../froglet-services/ops/cloudflare-workers/chatgpt-app/');
      expect(text).toContain('public endpoint is deployed');
      expect(text).toContain('demo.add` result `{ "sum": 12 }`');
      expect(text).toContain('not yet done: MCP Inspector');
      expect(text).toContain('Developer Mode');
      expect(text).toContain('Codex plugin');
      expect(text).toContain('Claude Code');
      expect(text).toContain('OpenClaw');
      expect(text).toContain('NemoClaw');
      expect(text).toContain('Third-party MCP directories');
      expect(text).toContain('public free `demo.*` services');
      expect(text).toContain('mcp-publisher publish');
      expect(text).toContain('Do not use `curl https://froglet.dev/learn/plugin-distribution/` as a proof');
      expect(text).toContain('node -e');
      expect(text).toContain('claude plugin marketplace add armanas/froglet --sparse .claude-plugin plugins');
    }
  });
});
