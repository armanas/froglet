import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const src = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const read = (path: string) => readFileSync(resolve(src, path), 'utf8');
const page = read('pages/marketplace.astro');
const script = read('scripts/marketplace-live.ts');
const css = read('styles/marketplace.css');

describe('Explore services page', () => {
  it('is built on the shared story layout with its own stylesheet and no page-level overrides', () => {
    expect(page).toContain("import StoryLayout from '../layouts/StoryLayout.astro'");
    expect(page).toContain("import '../styles/marketplace.css'");
    expect(page).toContain('<StoryLayout title="Explore services · Froglet"');
    expect(page).not.toContain('index-page.css');
    expect(page).not.toMatch(/<style[\s>]/);
  });

  it('has a single heading and a labelled search box', () => {
    expect(page.match(/<h1[\s>]/g)).toHaveLength(1);
    expect(page).toMatch(/<label class="sr-only" for="marketplace-search">/);
    expect(page).toContain('id="marketplace-search"');
    expect(page).toContain('data-marketplace-search-count');
  });

  it('announces status changes and labels its regions', () => {
    expect(page).toMatch(/data-marketplace-field="message" role="status"/);
    expect(page).toContain('aria-label="Catalog status"');
    expect(page).toContain('aria-label="Catalog totals"');
    expect(page).toContain('aria-label="Filters"');
    expect(page).toContain('<legend>Browse</legend>');
  });

  it('declares the sortable headers the script understands, sorted by status to begin with', () => {
    for (const key of ['name', 'category', 'price', 'status']) expect(page).toContain(`data-sort="${key}"`);
    expect(page).toMatch(/<th scope="col" class="mkt-col-status" aria-sort="ascending">/);
    expect(page.match(/<th scope="col" class="mkt-col-/g)).toHaveLength(5);
  });

  it('keeps its caveats next to the numbers they qualify', () => {
    expect(page).toContain('Opening a link does not run or pay for anything.');
    expect(page).toContain('Sample: ');
    expect(page).toContain('sampled indexed receipt totals');
    expect(page).toContain('reported success rate');
    expect(page).toContain('Receipts are provider statements, not independently assessed quality or unique buyers.');
    expect(page).toContain('Catalog counts do not measure customer activity.');
    expect(page).toContain('has not expired');
  });

  it('links its evidence sources and the receipt verifier', () => {
    expect(page).toContain('https://marketplace.froglet.dev/v1/providers?limit=12');
    expect(page).toContain('https://marketplace.froglet.dev/v1/offers?limit=24');
    expect(page).toContain('href="/verify-receipt/"');
  });
});

describe('marketplace stylesheet', () => {
  it('never removes a table column with display:none, which would invent a phantom column', () => {
    const rules = Array.from(css.matchAll(/([^{}]*mkt-col-[^{}]*)\{([^}]*)\}/g));
    expect(rules.length).toBeGreaterThan(0);
    for (const [, selector, body] of rules) {
      expect(body, selector.trim()).not.toMatch(/display\s*:\s*none/);
    }
  });

  it('collapses the low-priority columns to zero width and hides them from assistive technology', () => {
    const collapsing = Array.from(css.matchAll(/([^{}]*mkt-col-[^{}]*)\{([^}]*)\}/g)).filter(([, , body]) => /width:\s*0\b/.test(body));
    for (const column of ['type', 'price', 'status']) {
      const rule = collapsing.find(([, selector]) => selector.includes(`mkt-col-${column}`));
      expect(rule, `mkt-col-${column}`).toBeDefined();
      expect(rule![2]).toContain('visibility: hidden');
      expect(rule![2]).toContain('white-space: nowrap');
    }
  });

  it('respects reduced motion for every animation it defines', () => {
    const start = css.indexOf('@media (prefers-reduced-motion: reduce)');
    expect(start).toBeGreaterThan(-1);
    const rule = css.slice(start).match(/\{\s*([^{}]+?)\s*\{([^}]*)\}/);
    expect(rule).not.toBeNull();
    const [, selectors, body] = rule!;
    expect(body).toMatch(/animation:\s*none/);
    for (const animated of ['.mkt-skeleton', '.marketplace-status::before', '.mkt-item.is-updated .mkt-row', '.mkt-item.is-new .mkt-row']) {
      expect(selectors).toContain(animated);
    }
  });

  it('styles every mkt- class the page and the script create', () => {
    const fromPage = Array.from(page.matchAll(/class="([^"]+)"/g)).flatMap((match) => match[1].split(/\s+/));
    const fromScript = Array.from(script.matchAll(/\bmkt-[a-z0-9_-]*[a-z0-9]\b/g)).map((match) => match[0]);
    const classes = new Set([...fromPage, ...fromScript].filter((name) => name.startsWith('mkt-')));
    expect(classes.size).toBeGreaterThan(30);
    // Not classes: ids that share the prefix (the script builds `mkt-detail-<n>`), and the one flexible
    // column, which simply takes the width the fixed columns leave over.
    const notStyled = new Set(['mkt-list-heading', 'mkt-scope', 'mkt-detail-', 'mkt-col-service']);
    const unstyled = Array.from(classes).filter((name) => !notStyled.has(name) && !css.includes(`.${name}`));
    expect(unstyled).toEqual([]);
  });
});
