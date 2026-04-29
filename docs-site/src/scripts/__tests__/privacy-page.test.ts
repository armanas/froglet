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
  it('publishes a review-ready privacy URL for the website and ChatGPT app', () => {
    const page = readRepoFile('docs-site/src/pages/privacy.astro');
    const footer = readRepoFile('docs-site/src/components/SiteFooter.astro');
    const submission = readRepoFile('docs/PLUGIN_DISTRIBUTION.md');

    expect(page).toContain('<link rel="canonical" href="https://froglet.dev/privacy/" />');
    expect(page).toContain('Last updated: {UPDATED}');
    expect(page).toContain('Apache-2.0 open source protocol');
    expect(page).toContain('run_hosted_proof');
    expect(page).toContain('plan_local_install');
    expect(page).toContain('explain_use_case');
    expect(page).toContain('https://example.com/');
    expect(page).toContain('It should not use private user files');
    expect(page).not.toContain('MIT');
    expect(footer).toContain('<a href="/privacy/">Privacy</a>');
    expect(submission).toContain('https://froglet.dev/privacy/');
  });

  it('keeps secrets and paid-rail credentials out of hosted and ChatGPT flows', () => {
    const page = readRepoFile('docs-site/src/pages/privacy.astro');

    expect(page).toContain('Do not paste secrets');
    expect(page).toContain('private keys');
    expect(page).toContain('Lightning macaroons');
    expect(page).toContain('Stripe keys');
    expect(page).toContain('x402 credentials');
    expect(page).toContain('should not be pasted into the hosted app');
    expect(page).toContain('Froglet does not sell personal data');
  });
});
