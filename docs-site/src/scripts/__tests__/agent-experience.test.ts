import { describe, expect, it } from 'vitest';
import { existsSync, readFileSync } from 'node:fs';
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
    expect(manifest.default_task_id).toBe('publish-service');
    expect(taskIds).toEqual(expect.arrayContaining([
      'publish-service',
      'consume-service',
      'bounded-compute',
      'receipt-artifact-verify',
      'marketplace-evidence',
      'local-install-proposal',
      'chat-only-fallback',
    ]));

    // The first-party hosted trial was retired on 8 October 2026; no task may send an agent there.
    for (const retired of ['hosted-proof', 'hosted-proof-with-witness', 'receipt-feed-check']) expect(taskIds).not.toContain(retired);
    expect(JSON.stringify(manifest)).not.toMatch(/try\.froglet\.dev|ai\.froglet\.dev/);
    const install = manifest.task_contracts.find((task) => task.task_id === 'local-install-proposal');
    expect(install?.expected_report_fields).not.toContain('hosted_evidence_complete');
    const fallback = manifest.task_contracts.find((task) => task.task_id === 'chat-only-fallback');
    expect(fallback?.must_not_claim).toContain('live Froglet evidence');

    const compute = manifest.task_contracts.find((task) => task.task_id === 'bounded-compute');
    expect(compute?.entrypoint).toBe('https://froglet.dev/learn/agent-interoperability/');
    expect(compute?.expected_report_fields).toContain('receipt_verification');
    expect(compute?.must_not_claim).toContain('a submission_pending reference proves provider admission');
  });

  it('separates the root task router from the optional hosted proof', () => {
    const canonical = readRepoFile('docs/llms/try.froglet.dev.txt');
    const publicCopy = readRepoFile('docs-site/public/llms.txt');
    const cloudTrial = readRepoFile('docs-site/src/content/docs/learn/cloud-trial.mdx');

    expect(publicCopy).toContain('https://froglet.dev/publish/agent.md');
    expect(publicCopy).toContain('not a prerequisite');
    expect(canonical).not.toContain('finish the hosted proof first');
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

  it('keeps route-level agent metadata on the docs-hosted walkthrough', () => {
    const head = readRepoFile('docs-site/src/components/StarlightHead.astro');
    const config = readRepoFile('docs-site/astro.config.mjs');
    const metadata = readRepoFile('docs-site/src/data/agent-metadata.ts');

    // The walkthrough is a docs entry at /demo/, so its metadata comes from the Starlight Head override.
    expect(head).toContain('AgentMeta');
    expect(head).toContain('routeAgentMetadata');
    expect(config).toContain("Head: './src/components/StarlightHead.astro'");
    expect(metadata).toContain("route: '/demo/'");
    expect(readRepoFile('docs-site/src/content/docs/demo.mdx')).toContain('ProtocolWalkthrough');
    // A standalone page at the same route would shadow the docs entry.
    expect(existsSync(resolve(repoRoot, 'docs-site/src/pages/demo.astro'))).toBe(false);
  });

  it('adds local receipt verification with explicit limits', () => {
    const page = readRepoFile('docs-site/src/pages/verify-receipt.astro');
    const footer = readRepoFile('docs-site/src/components/SiteFooter.astro');

    expect(page).toContain('data-receipt-input');
    expect(page).toContain('data-receipt-verify');
    expect(page).toContain('receipt-artifact-verify');
    expect(page).toContain('initReceiptVerifier');
    expect(page).toContain('They do not prove a human or organization authorized the action');
    expect(footer).toContain('/verify-receipt/');
  });

  it('adds marketplace evidence copy actions without implying paid hosted rails', () => {
    const page = readRepoFile('docs-site/src/pages/marketplace.astro');
    const script = readRepoFile('docs-site/src/scripts/marketplace-live.ts');

    expect(page).toContain('data-marketplace-copy-summary');
    expect(page).toContain('data-marketplace-copy-provider');
    expect(script).toContain('providerSummary');
    expect(page).toContain('/verify-receipt/');
    expect(script).toContain('marketplaceEvidence');
    expect(script).toContain('not_proved: hosted paid rails');
    expect(script).toContain('providerSummary');
    expect(script).toContain('Copy failed');
  });
});
