import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { navLinks } from '../../data/nav-links';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '../../../..');

function readRepoFile(path: string): string {
  return readFileSync(resolve(repoRoot, path), 'utf8');
}

describe('protocol walkthrough in the documentation', () => {
  it('is a docs entry at /demo/ listed in the Concepts sidebar', () => {
    expect(existsSync(resolve(repoRoot, 'docs-site/src/content/docs/demo.mdx'))).toBe(true);
    const config = readRepoFile('docs-site/astro.config.mjs');
    expect(config).toContain("{ label: 'Protocol Walkthrough', slug: 'demo' }");
    expect(config.indexOf("label: 'Concepts'")).toBeLessThan(config.indexOf("slug: 'demo'"));
    expect(config.indexOf("slug: 'demo'")).toBeLessThan(config.indexOf("label: 'Reference'"));
  });

  it('keeps the Docs header item active on /demo/', () => {
    const docs = navLinks.find((item) => item.href === '/docs/');
    expect(docs && 'activePrefixes' in docs ? docs.activePrefixes : []).toContain('/demo/');
  });

  it('is linked under one name from the docs, footer and 404 page', () => {
    for (const path of [
      'docs-site/src/content/docs/docs.mdx',
      'docs-site/src/content/docs/learn/index.mdx',
    ]) {
      const page = readRepoFile(path);
      expect(page, path).toContain('href="/demo/"');
      expect(page, path).toContain('Protocol Walkthrough');
      expect(page, path).not.toContain('Watch Demo');
    }
    expect(readRepoFile('docs-site/src/components/SiteFooter.astro')).toContain('href="/demo/"');
    expect(readRepoFile('docs-site/src/pages/404.astro')).toContain('href="/demo/"');
  });

  it('keeps the element ids that the walkthrough script binds to', () => {
    const component = readRepoFile('docs-site/src/components/ProtocolWalkthrough.astro');
    for (const hook of [
      'data-walkthrough',
      'id="scene"',
      'id="net"',
      'id="lesson-card"',
      'id="annotation"',
      'id="term-body"',
      'id="pips"',
      'id="prevBtn"',
      'id="nextBtn"',
    ]) {
      expect(component, hook).toContain(hook);
    }
  });

  it('is a scoped widget that does not lock or restyle the whole page', () => {
    const css = readRepoFile('docs-site/src/styles/walkthrough.css');
    expect(css).not.toMatch(/^\s*(html|body|\*)\s*[,{]/m);
    // Every rule other than the root-level width override is scoped to the widget.
    const selectors = [...css.matchAll(/^([^\s@/}][^{]*)\{/gm)].map((match) => match[1].trim());
    for (const selector of selectors) {
      expect(selector.startsWith('.walkthrough') || selector.startsWith(':root:has(.walkthrough)') || selector.startsWith("html[data-theme='light'] .walkthrough")).toBe(true);
    }
  });
});
