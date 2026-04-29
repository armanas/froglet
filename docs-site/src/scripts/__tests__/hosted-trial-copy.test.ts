import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '../../../..');

function readRepoFile(path: string): string {
  return readFileSync(resolve(repoRoot, path), 'utf8');
}

const strongPrompt =
  'Read https://try.froglet.dev/llms.txt, follow the hosted demo flow exactly if you can access it, otherwise say only that you could not access it, then give me an honest, evidence-backed assessment that reports the observed HTTP statuses, observed service IDs, observed deal status, observed result, whether a receipt was present, and any mismatch between these docs and live behavior before explaining what Froglet just proved, what it did not prove, and the single most relevant next experiment for my files, tools, data, configuration, workflows, constraints, and goals.';

const homepagePrompt =
  'Open https://try.froglet.dev/llms.txt. Run demo.add and one witness/hash follow-up. Report statuses, result, receipt/feed proof, mismatches, and what this proves or does not prove for my projects/data. If unreachable, say so.';

const hostedCopyFiles = [
  'docs/HOSTED_TRIAL.md',
  'docs-site/src/content/docs/learn/cloud-trial.mdx',
  'docs/llms/try.froglet.dev.txt',
];

const demoServices = [
  'demo.add',
  'demo.echo',
  'demo.fetch-witness',
  'demo.hash-verify',
  'demo.notarize',
];

describe('hosted trial docs copy', () => {
  it('uses the same evidence-backed public prompt everywhere', () => {
    for (const path of hostedCopyFiles) {
      expect(readRepoFile(path), path).toContain(strongPrompt);
    }
  });

  it('uses a compact prompt on the homepage hero', () => {
    const index = readRepoFile('docs-site/src/pages/index.astro');
    expect(index).toContain(homepagePrompt);
    expect(index).toContain('live evidence, not a product summary');
    expect(index).not.toContain(strongPrompt);
  });

  it('documents the five free hosted demo services', () => {
    for (const path of [
      'docs/HOSTED_TRIAL.md',
      'docs-site/src/content/docs/learn/cloud-trial.mdx',
      'docs/llms/try.froglet.dev.txt',
    ]) {
      const text = readRepoFile(path);
      for (const service of demoServices) {
        expect(text, `${path} should mention ${service}`).toContain(service);
      }
    }
  });

  it('gives LLMs complete hosted deal bodies instead of a minimal body guess', () => {
    const llms = readRepoFile('docs/llms/try.froglet.dev.txt');
    expect(llms).toContain('Do not invent a shorter');
    expect(llms).toContain('PROVIDER_ID_FROM_CATALOG');
    expect(llms).toContain('"provider":{"provider_id":"PROVIDER_ID_FROM_CATALOG","provider_url":"https://ai.froglet.dev"}');
    expect(llms).toContain('"offer_id":"demo.add"');
    expect(llms).toContain('"input":{"a":7,"b":5}');
    expect(llms).toContain('"offer_id":"demo.fetch-witness"');
    expect(llms).toContain('"input":{"url":"https://example.com/","max_bytes":1048576}');
    expect(llms).toContain('18020a87586eb7e41683ff11bca3fb67398f123b4bbb8786434797cf2a9affbc');
  });

  it('documents preflight, authorized scope, and demo-only hosted proof boundaries', () => {
    for (const path of [
      'docs/HOSTED_TRIAL.md',
      'docs-site/src/content/docs/learn/cloud-trial.mdx',
      'docs/llms/try.froglet.dev.txt',
    ]) {
      const text = readRepoFile(path);
      expect(text, `${path} should document preflight`).toContain('GET /api/preflight');
      expect(text, `${path} should require POST JSON capability`).toContain('POST JSON');
      expect(text, `${path} should require Bearer auth capability`).toContain('Bearer auth');
      expect(text, `${path} should mention authorized scope`).toContain('Authorized scope');
      expect(text, `${path} should prohibit scanning`).toContain('Do not scan');
      expect(text, `${path} should scope hosted proof to demo services`).toContain('Only `demo.*` services are part of the public hosted proof');
      expect(text, `${path} should exclude non-demo services`).toMatch(/Other service IDs\s+may appear/);
    }
  });

  it('gives agents a failure taxonomy that separates tool limits from service failures', () => {
    for (const path of [
      'docs/HOSTED_TRIAL.md',
      'docs-site/src/content/docs/learn/cloud-trial.mdx',
      'docs/llms/try.froglet.dev.txt',
    ]) {
      const text = readRepoFile(path);
      expect(text, `${path} should classify preflight/tool limits`).toContain('client/tool limitation');
      expect(text, `${path} should classify wrong session method`).toContain('GET /api/sessions');
      expect(text, `${path} should classify missing auth`).toContain('401');
      expect(text, `${path} should classify egress policy blocks`).toContain('host_not_allowed');
      expect(text, `${path} should classify pool exhaustion`).toContain('session pool exhaustion');
      expect(text, `${path} should classify deal failures`).toContain('failed');
    }
  });

  it('keeps demo.add canonical and stronger demos optional', () => {
    for (const path of [
      'docs/HOSTED_TRIAL.md',
      'docs-site/src/content/docs/learn/cloud-trial.mdx',
      'docs/llms/try.froglet.dev.txt',
    ]) {
      const text = readRepoFile(path);
      expect(text, `${path} should name demo.add as canonical`).toMatch(/demo\.add[\s\S]{0,120}Canonical proof|Canonical proof[\s\S]{0,120}demo\.add/);
      expect(text, `${path} should make fetch-witness optional`).toMatch(/demo\.fetch-witness[\s\S]{0,120}Optional stronger follow-up|Optional stronger follow-up[\s\S]{0,120}demo\.fetch-witness/);
      expect(text, `${path} should make hash-verify optional`).toMatch(/demo\.hash-verify[\s\S]{0,120}Optional stronger follow-up|Optional stronger follow-up[\s\S]{0,120}demo\.hash-verify/);
      expect(text, `${path} should make notarize optional`).toMatch(/demo\.notarize[\s\S]{0,120}Optional stronger follow-up|Optional stronger follow-up[\s\S]{0,120}demo\.notarize/);
    }
  });

  it('defines /v1/feed as an artifact envelope, not events or items', () => {
    for (const path of [
      'docs/HOSTED_TRIAL.md',
      'docs-site/src/content/docs/learn/cloud-trial.mdx',
      'docs/llms/try.froglet.dev.txt',
    ]) {
      const text = readRepoFile(path);
      expect(text, `${path} should define feed as artifact envelope`).toContain('artifact envelope');
      expect(text, `${path} should mention descriptors`).toContain('descriptors');
      expect(text, `${path} should mention offers`).toContain('offers');
      expect(text, `${path} should mention receipts`).toContain('receipts');
      expect(text, `${path} should reject event-stream interpretation`).toContain('events stream');
      expect(text, `${path} should reject items-collection interpretation`).toContain('items');
      expect(text, `${path} should clarify feed matching is artifact-keyed`).toContain('runtime `deal_id`');
      expect(text, `${path} should mention deal_hash`).toContain('deal_hash');
      expect(text, `${path} should mention quote_hash`).toContain('quote_hash');
    }
  });

  it('does not imply hosted Lightning payment in the homepage proof strip', () => {
    const index = readRepoFile('docs-site/src/pages/index.astro');
    expect(index).toContain('proof-strip');
    expect(index).toContain('5 free demos');
    expect(index).toContain('receipt + feed');
    expect(index).not.toContain('500 sats');
    expect(index).not.toContain('600 sats');
    expect(index).not.toContain('paid ·');
  });

  it('keeps marketplace metrics from implying hosted paid rails are live', () => {
    const marketplace = readRepoFile('docs-site/src/pages/marketplace.astro');
    expect(marketplace).toContain('Receipt value');
    expect(marketplace).toContain('indexed receipt totals');
    expect(marketplace).toContain('priced');
    expect(marketplace).toContain('data-marketplace-search');
    expect(marketplace).toContain('data-marketplace-search-row');
    expect(marketplace).not.toContain('Volume settled');
    expect(marketplace).not.toContain('lightning + stripe + x402');
  });

  it('lists batch and GPU without claiming hosted GPU is live', () => {
    const index = readRepoFile('docs-site/src/pages/index.astro');
    expect(index).toContain('Batch');
    expect(index).toContain('async job queue');
    expect(index).toContain('GPU');
    expect(index).toContain('provider advertises');
    expect(index).toContain('Planned');
  });

  it('keeps the README aligned with the deployed five-service catalog', () => {
    const readme = readRepoFile('README.md');
    for (const service of demoServices) {
      expect(readme, `README should mention ${service}`).toContain(service);
    }
    expect(readme).toContain('The hosted trial still does not prove paid rails');
    expect(readme).not.toContain('trial proves only the\n> free `demo.add`');
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

  it('tells LLMs to stage local install instead of jumping from proof to shell commands', () => {
    for (const path of [
      'docs/HOSTED_TRIAL.md',
      'docs-site/src/content/docs/learn/cloud-trial.mdx',
      'docs-site/src/content/docs/learn/llm-self-install.mdx',
      'docs/llms/try.froglet.dev.txt',
    ]) {
      const text = readRepoFile(path);
      expect(text, `${path} should mention plan_install`).toContain('plan_install');
      expect(text, `${path} should mention get_install_guide`).toContain('get_install_guide');
      expect(text, `${path} should include network choice`).toContain('tor');
      expect(text, `${path} should include local footprint choice`).toContain('docker');
    }
  });

  it('gives simple chat LLMs a truthful fallback when they cannot run tools', () => {
    const llms = readRepoFile('docs/llms/try.froglet.dev.txt');
    const cloud = readRepoFile('docs-site/src/content/docs/learn/cloud-trial.mdx');
    for (const text of [llms, cloud]) {
      expect(text).toContain('cannot run the Froglet hosted proof');
      expect(text).toContain('chat interface');
      expect(text).toContain('curl');
      expect(text).toContain('must not claim');
    }
  });

  it('documents the published MCP package on the website and repo docs', () => {
    const paths = [
      'docs-site/src/content/docs/learn/agents.mdx',
      'docs-site/src/content/docs/learn/cloud-trial.mdx',
      'docs-site/src/content/docs/learn/llm-self-install.mdx',
      'docs-site/src/content/docs/learn/quickstart.mdx',
      'docs/HOSTED_TRIAL.md',
      'docs/llms/try.froglet.dev.txt',
      'README.md',
      'docs/CONFIGURATION.md',
    ];

    for (const path of paths) {
      const text = readRepoFile(path);
      expect(text, `${path} should mention the npm package`).toContain('npx froglet-mcp');
      expect(text, `${path} should mention hosted proof action`).toContain('run_hosted_proof');
    }
  });

  it('keeps MCP hosted-proof and local-profile boundaries explicit', () => {
    const agents = readRepoFile('docs-site/src/content/docs/learn/agents.mdx');
    const config = readRepoFile('docs/CONFIGURATION.md');
    const readme = readRepoFile('README.md');

    for (const text of [agents, config, readme]) {
      expect(text).toContain('FROGLET_PROFILE=hosted-proof');
      expect(text).toContain('FROGLET_PROFILE=local');
      expect(text).toContain('FROGLET_PROVIDER_AUTH_TOKEN_PATH');
      expect(text).toContain('FROGLET_RUNTIME_AUTH_TOKEN_PATH');
    }

    expect(config).toContain('do not require local token files');
    expect(config).toContain('Default search result limit');
    expect(config).toContain('`10`');
    expect(config).toContain('`50`');
  });
});
