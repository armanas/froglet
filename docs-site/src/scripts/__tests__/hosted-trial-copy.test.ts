import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '../../../..');

function readRepoFile(path: string): string {
  return readFileSync(resolve(repoRoot, path), 'utf8');
}

// The retired hosted trial's prompt; it must not reappear as an instruction.
const retiredPrompt = 'Read https://try.froglet.dev/llms.txt, follow the hosted demo flow exactly';

const retirementFiles = [
  'docs/HOSTED_TRIAL.md',
  'docs-site/src/content/docs/learn/cloud-trial.mdx',
  'docs/llms/try.froglet.dev.txt',
];

describe('hosted trial retirement and public docs copy', () => {
  it('marks the first-party hosted trial retired wherever agents and readers look', () => {
    for (const path of retirementFiles) {
      const text = readRepoFile(path);
      expect(text, path).toMatch(/retired on 8 October\s+2026/);
      expect(text, path).not.toContain(retiredPrompt);
      expect(text, path).not.toContain('POST /api/sessions');
      expect(text, path).toContain('froglet.dev/services/');
    }
  });

  it('keeps setup on the publishing page and optional hosted tasks in their guide', () => {
    const index = readRepoFile('docs-site/src/pages/index.astro');
    const publish = readRepoFile('docs-site/src/pages/publish.astro');
    expect(index).toContain('href="/publish/"');
    expect(index).not.toContain('try.froglet.dev');
    expect(publish).toContain('show a preview before asking to publish');
    expect(publish).toContain('copy-publisher-prompt');
    expect(publish).toContain("new URL('/publish/agent.md', siteOrigin)");
    expect(publish).toContain('Ask me separately before persistent installation and before publishing');
    expect(publish).toContain('Preserve my existing files and agent settings');
    expect(index).not.toContain(retiredPrompt);
  });

  it('routes agents away from the retired trial instead of to it', () => {
    const root = readRepoFile('docs-site/public/llms.txt');
    expect(root).toContain('https://froglet.dev/publish/agent.md');
    expect(root).not.toContain('https://try.froglet.dev/llms.txt');
    expect(root).toMatch(/try\.froglet\.dev was retired/);
    expect(root).not.toContain('finish the hosted proof first');
    expect(root).not.toContain('POST /api/sessions');
  });

  it('tells agents reading the retired trial what to say and where to go', () => {
    const llms = readRepoFile('docs/llms/try.froglet.dev.txt');
    expect(llms).toContain('You must not claim a successful hosted proof');
    expect(llms).toContain('https://froglet.dev/agent-tasks.json');
    expect(llms).toContain('receipt-artifact-verify');
    expect(llms).toContain('marketplace-evidence');
    for (const removed of ['PROVIDER_ID_FROM_CATALOG', '"offer_id":"demo.add"', 'Bearer']) expect(llms).not.toContain(removed);
  });

  it('does not confuse the public bounded beta with the legacy five-service proof', () => {
    const index = readRepoFile('docs-site/src/pages/index.astro');
    const developers = readRepoFile('docs-site/src/pages/open-source.astro');
    expect(index).not.toContain('proof-strip');
    expect(index).not.toContain('5 free demos');
    expect(developers).toContain('runs supplied Wasm programs and a synthetic catalog without installation or an invitation');
    expect(index).not.toContain('500 sats');
    expect(index).not.toContain('600 sats');
    expect(index).not.toContain('paid ·');
  });

  it('keeps payment rails framed as operator-only, not buyer onboarding', () => {
    const paymentRails = readRepoFile('docs-site/src/content/docs/learn/payment-rails.mdx');
    const quickstart = readRepoFile('docs-site/src/content/docs/learn/quickstart.mdx');
    const index = readRepoFile('docs-site/src/pages/index.astro');

    expect(paymentRails).toContain('End users should not configure Lightning nodes');
    expect(paymentRails).toMatch(/Do not present it as normal customer\s+onboarding/);
    expect(quickstart).toContain('Ordinary buyers should not configure LND');
    expect(quickstart).toMatch(/Pick `none` for the first demo and for normal\s+customer evaluation/);
    const setup = readRepoFile('docs-site/src/components/OperatorSetup.astro');
    expect(setup).toContain('Pick an agent and start free');
    expect(setup).toContain('Normal users should keep this at None');
    expect(index).not.toContain('data-group="payment"');
    expect(index).not.toContain('Pick an agent and the first payment decision');
  });

  it('keeps marketplace metrics from implying hosted paid rails are live', () => {
    const marketplace = readRepoFile('docs-site/src/pages/marketplace.astro');
    expect(marketplace).toContain('Receipt value');
    expect(marketplace).toContain('indexed receipt totals');
    expect(marketplace).toContain('priced');
    expect(marketplace).toContain('data-marketplace-search');
    expect(readRepoFile('docs-site/src/scripts/marketplace-live.ts')).toContain('marketplaceSearchRow');
    expect(marketplace).not.toContain('Volume settled');
    expect(marketplace).not.toContain('lightning + stripe + x402');
  });

  it('routes advanced runtimes to their guide without claiming hosted GPU is live', () => {
    const index = readRepoFile('docs-site/src/pages/index.astro');
    const developers = readRepoFile('docs-site/src/pages/open-source.astro');
    const scope = readRepoFile('docs-site/src/content/docs/learn/cloud-trial.mdx');
    expect(developers).toContain('current batch and GPU constraints');
    expect(developers).toContain('href="/learn/cloud-trial/"');
    expect(developers).toContain('Tor is an advanced self-hosted path');
    expect(developers).toContain('Confidential execution and selective disclosure remain specifications');
    expect(scope).toContain('GPU execution is currently refused');
    for (const page of [index, developers, scope]) {
      expect(page).not.toContain('T4 verified');
      expect(page).not.toContain('GPU is live');
    }
  });

  it('points README readers to the public beta and the retirement record', () => {
    const readme = readRepoFile('README.md');
    expect(readme).toContain('https://froglet.dev/services/');
    expect(readme).toContain('docs/HOSTED_TRIAL.md');
    expect(readme).not.toContain('Try In Cloud');
    expect(readme).not.toContain('Session tokens on `try.froglet.dev`');
  });

  it('labels the website license as Apache-2.0, not MIT', () => {
    const footer = readRepoFile('docs-site/src/components/SiteFooter.astro');
    expect(footer).toContain('Apache-2.0 licensed');
    expect(footer).toContain('https://armanas.dev');
    expect(footer).toContain('Built by');
    expect(footer).not.toContain('MIT' + ' licensed');
  });

  it('keeps copyable website blocks on the same black copy surface', () => {
    const tokens = readRepoFile('docs-site/src/styles/tokens.css');
    const starlight = readRepoFile('docs-site/src/styles/starlight-overrides.css');
    const index = readRepoFile('docs-site/src/styles/index-page.css');
    const components = readRepoFile('docs-site/src/styles/components.css');

    expect(tokens).toContain('--copy-bg: #000');
    expect(tokens).toContain('--copy-text: #f8fff4');
    expect(starlight).toContain('.expressive-code pre');
    expect(starlight).toContain('background: var(--copy-bg) !important');
    expect(starlight).toContain('.expressive-code code span');
    expect(starlight).toContain('color: var(--copy-text) !important');
    expect(starlight).toContain('.expressive-code .copy button');
    expect(index).toContain('.hero-agent .hero-prompt');
    expect(index).toContain('background: var(--copy-bg)');
    expect(index).toContain('.config-shell');
    expect(components).toContain('.learn-code-box pre');
    expect(components).toContain('background: var(--copy-bg)');
  });

  it('tells LLMs to stage local install instead of jumping to shell commands', () => {
    for (const path of [
      'docs-site/src/content/docs/learn/cloud-trial.mdx',
      'docs-site/src/content/docs/learn/llm-self-install.mdx',
    ]) {
      const text = readRepoFile(path);
      expect(text, `${path} should mention plan_install`).toContain('plan_install');
      expect(text, `${path} should mention get_install_guide`).toContain('get_install_guide');
      expect(text, `${path} should mention plan_use_case`).toContain('plan_use_case');
      expect(text, `${path} should include network choice`).toContain('tor');
      expect(text, `${path} should include local footprint choice`).toContain('docker');
    }
    const retired = readRepoFile('docs/llms/try.froglet.dev.txt');
    for (const tool of ['plan_install', 'get_install_guide', 'plan_use_case']) expect(retired).toContain(tool);
  });

  it('gives simple chat LLMs a truthful fallback when they cannot run tools', () => {
    const cloud = readRepoFile('docs-site/src/content/docs/learn/cloud-trial.mdx');
    expect(cloud).toContain('I cannot run Froglet from this chat interface');
    expect(cloud).toContain('https://froglet.dev/services/');
    expect(cloud).toContain('must not claim');
    expect(cloud).not.toContain('cannot run the Froglet hosted proof');
  });

  it('documents the published MCP package as local/actionable, not a demo wrapper', () => {
    const paths = [
      'docs-site/src/content/docs/learn/agents.mdx',
      'docs-site/src/content/docs/learn/llm-self-install.mdx',
      'docs-site/src/content/docs/learn/quickstart.mdx',
      'README.md',
      'docs/CONFIGURATION.md',
    ];

    for (const path of paths) {
      const text = readRepoFile(path);
      expect(text, `${path} should mention the npm package`).toContain('npx froglet-mcp');
      expect(text, `${path} should point no-install proof to llms.txt`).toContain('froglet.dev/llms.txt');
    }
  });

  it('keeps MCP local-profile boundaries explicit', () => {
    const agents = readRepoFile('docs-site/src/content/docs/learn/agents.mdx');
    const config = readRepoFile('docs/CONFIGURATION.md');
    const readme = readRepoFile('README.md');

    for (const text of [agents, config, readme]) {
      expect(text).toContain('FROGLET_PROFILE=local');
      expect(text).toContain('FROGLET_PROVIDER_AUTH_TOKEN_PATH');
      expect(text).toContain('FROGLET_RUNTIME_AUTH_TOKEN_PATH');
      expect(text).not.toContain('FROGLET_PROFILE=hosted-proof');
    }

    expect(config).toContain('do not require local token files');
    expect(config).toContain('Default search result limit');
    expect(config).toContain('`10`');
    expect(config).toContain('`50`');
  });
});
