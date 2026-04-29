import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '../../../..');

function readRepoFile(path: string): string {
  return readFileSync(resolve(repoRoot, path), 'utf8');
}

describe('privacy page', () => {
  it('publishes a privacy URL for the website, hosted proof, and local MCP boundary', () => {
    const page = readRepoFile('docs-site/src/pages/privacy.astro');
    const footer = readRepoFile('docs-site/src/components/SiteFooter.astro');
    const submission = readRepoFile('docs/PLUGIN_DISTRIBUTION.md');

    expect(page).toContain('<link rel="canonical" href="https://froglet.dev/privacy/" />');
    expect(page).toContain('Last updated: {UPDATED}');
    expect(page).toContain('Apache-2.0 open source protocol');
    expect(page).toContain('plan_install');
    expect(page).toContain('publish_artifact');
    expect(page).toContain('local/self-hosted usage');
    expect(page).toContain('Do not paste secrets');
    expect(page).not.toContain('ChatGPT app');
    expect(page).not.toContain('run_hosted_proof');
    expect(page).not.toContain('MIT');
    expect(footer).toContain('<a href="/privacy/">Privacy</a>');
    expect(submission).toContain('local/actionable boundary');
  });

  it('keeps secrets and paid-rail credentials out of hosted and ChatGPT flows', () => {
    const page = readRepoFile('docs-site/src/pages/privacy.astro');

    expect(page).toContain('Do not paste secrets');
    expect(page).toContain('private keys');
    expect(page).toContain('Lightning macaroons');
    expect(page).toContain('Stripe keys');
    expect(page).toContain('x402 credentials');
    expect(page).toContain('should not be pasted into the hosted demo');
    expect(page).toContain('Froglet does not sell personal data');
  });
});
