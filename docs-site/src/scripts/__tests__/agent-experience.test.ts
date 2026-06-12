import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '../../../..');

function readRepoFile(path: string): string {
  return readFileSync(resolve(repoRoot, path), 'utf8');
}

function readJson<T>(path: string): T {
  return JSON.parse(readRepoFile(path)) as T;
}

interface AgentTaskManifest {
  schema_version: string;
  default_task_id: string;
  task_contracts: Array<{
    task_id: string;
    entrypoint: string;
    expected_report_fields: string[];
    must_not_claim: string[];
  }>;
}

describe('agent-facing website experience', () => {
  it('publishes a machine-readable task manifest for agents', () => {
    const manifest = readJson<AgentTaskManifest>('docs-site/public/agent-tasks.json');
    const taskIds = manifest.task_contracts.map((task) => task.task_id);

    expect(manifest.schema_version).toBe('froglet.agent-tasks.v1');
    expect(manifest.default_task_id).toBe('hosted-proof');
    expect(taskIds).toEqual(expect.arrayContaining([
      'hosted-proof',
      'hosted-proof-with-witness',
      'receipt-feed-check',
      'receipt-artifact-verify',
      'marketplace-evidence',
      'local-install-proposal',
      'chat-only-fallback',
    ]));

    const hostedProof = manifest.task_contracts.find((task) => task.task_id === 'hosted-proof');
    expect(hostedProof?.entrypoint).toBe('https://try.froglet.dev/llms.txt');
    expect(hostedProof?.expected_report_fields).toContain('docs_live_mismatches');
    expect(hostedProof?.must_not_claim).toContain('paid Lightning, Stripe, or x402 settlement');
  });

  it('keeps llms.txt canonical and points agents to the task manifest', () => {
    const canonical = readRepoFile('docs/llms/try.froglet.dev.txt');
    const publicCopy = readRepoFile('docs-site/public/llms.txt');
    const cloudTrial = readRepoFile('docs-site/src/content/docs/learn/cloud-trial.mdx');

    expect(publicCopy).toEqual(canonical);
    for (const text of [canonical, publicCopy, cloudTrial]) {
      expect(text).toContain('/agent-tasks.json');
      expect(text).toContain('receipt-artifact-verify');
      expect(text).toContain('marketplace-evidence');
    }
  });

  it('adds route-level agent metadata to custom pages', () => {
    const component = readRepoFile('docs-site/src/components/AgentMeta.astro');
    const metadata = readRepoFile('docs-site/src/data/agent-metadata.ts');

    expect(component).toContain('data-agent-route-metadata');
    expect(component).toContain('/agent-tasks.json');
    expect(metadata).toContain("primary_task: 'marketplace-evidence'");
    expect(metadata).toContain("primary_task: 'receipt-artifact-verify'");

    const pages = new Map([
      ['docs-site/src/pages/index.astro', 'route="home"'],
      ['docs-site/src/pages/marketplace.astro', 'route="marketplace"'],
      ['docs-site/src/pages/demo.astro', 'route="demo"'],
      ['docs-site/src/pages/managed.astro', 'route="managed"'],
      ['docs-site/src/pages/open-source.astro', 'route="openSource"'],
      ['docs-site/src/pages/privacy.astro', 'route="privacy"'],
      ['docs-site/src/pages/verify-receipt.astro', 'route="verifyReceipt"'],
    ]);

    for (const [path, routeAttribute] of pages) {
      const page = readRepoFile(path);
      expect(page, `${path} should import AgentMeta`).toContain('AgentMeta');
      expect(page, `${path} should declare its agent route`).toContain(routeAttribute);
    }
  });

  it('adds receipt verification as structural inspection with explicit limits', () => {
    const page = readRepoFile('docs-site/src/pages/verify-receipt.astro');
    const footer = readRepoFile('docs-site/src/components/SiteFooter.astro');

    expect(page).toContain('data-receipt-input');
    expect(page).toContain('data-receipt-verify');
    expect(page).toContain('receipt-artifact-verify');
    expect(page).toContain('not_cryptographically_verified');
    expect(page).toContain('signature validity, canonical hash, provider identity, and settlement remain unverified');
    expect(footer).toContain('/verify-receipt/');
  });

  it('adds marketplace evidence copy actions without implying paid hosted rails', () => {
    const page = readRepoFile('docs-site/src/pages/marketplace.astro');
    const script = readRepoFile('docs-site/src/scripts/marketplace-live.ts');

    expect(page).toContain('data-marketplace-copy-summary');
    expect(page).toContain('data-marketplace-copy-provider');
    expect(page).toContain('data-provider-summary');
    expect(page).toContain('/verify-receipt/');
    expect(script).toContain('marketplaceEvidence');
    expect(script).toContain('not_proved: hosted paid rails');
    expect(script).toContain('providerSummary');
    expect(script).toContain('Copy failed');
  });
});
