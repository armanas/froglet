import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const siteRoot = resolve(here, '../../..');

function read(path: string): string {
  return readFileSync(resolve(siteRoot, path), 'utf8');
}

const home = read('src/pages/index.astro');

/** The text of the requirement chips, in order. */
function heroRequirements(): string[] {
  const block = /<div class="story-needs"[\s\S]*?<\/ul>/.exec(home)?.[0] ?? '';
  return [...block.matchAll(/<li>([^<]+)<\/li>/g)].map((match) => match[1]);
}

describe('homepage requirements line', () => {
  it('states what a visitor needs, in the hero, right under the buttons', () => {
    const copy = home.indexOf('class="story-hero__copy"');
    const actions = home.indexOf('class="story-actions"', copy);
    const note = home.indexOf('class="story-note"', actions);
    const needs = home.indexOf('class="story-needs"', actions);
    const art = home.indexOf('class="story-hero__art', copy);
    expect(copy).toBeGreaterThan(-1);
    expect([copy, actions, note, needs, art]).toEqual([...[copy, actions, note, needs, art]].sort((a, b) => a - b));
    expect(heroRequirements()).toEqual(['Codex or Claude Code', 'Apple Silicon macOS or Linux', 'A computer that stays online']);
  });

  it('is a named group with list semantics for assistive technology', () => {
    const labelId = /class="story-needs" role="group" aria-labelledby="([^"]+)"/.exec(home)?.[1];
    expect(labelId).toBeTruthy();
    expect(home).toContain(`id="${labelId}">Today you’ll need</span>`);
    // list-style: none removes list semantics in some browsers unless the role is explicit
    expect(home).toMatch(/<div class="story-needs"[\s\S]*?<ul role="list">/);
  });

  it('says only what the docs say is supported today', () => {
    const quickstart = read('src/content/docs/learn/quickstart.mdx');
    const publish = read('src/pages/publish.astro');
    const chips = heroRequirements().join(' | ');

    // Source of truth for the platform and agent requirements is the quickstart.
    const supported = /\*\*Supported:\*\*([^\n]+)/.exec(quickstart)?.[1] ?? '';
    const agent = /\*\*Agent:\*\*([^\n]+)/.exec(quickstart)?.[1] ?? '';
    expect(supported, 'quickstart no longer states supported platforms; review the homepage chips').toMatch(/Apple Silicon/);
    expect(supported).toMatch(/Linux/);
    expect(supported, 'the docs added a platform; update the homepage chips to match').not.toMatch(/Windows|Intel/i);
    expect(agent).toMatch(/Claude Code/);
    expect(agent).toMatch(/Codex/);

    expect(chips).toContain('Apple Silicon');
    expect(chips).toContain('Linux');
    expect(chips).toContain('Codex');
    expect(chips).toContain('Claude Code');
    expect(chips).not.toMatch(/Windows|Intel/i);

    // The publish page and the FAQ on this page make the same statements.
    expect(publish).toContain('Apple Silicon macOS and Linux');
    expect(publish).toContain('Codex or Claude Code');
    expect(home).toContain('such as Codex or Claude Code on Apple Silicon macOS or Linux');
    expect(home).toContain('needs to stay awake, online, and running Froglet');
  });

  it('keeps the beta status note above the requirements', () => {
    expect(home.indexOf('Public beta · Start with a free service on your own computer.')).toBeLessThan(
      home.indexOf('class="story-needs"'),
    );
  });
});

describe('homepage layout on phones', () => {
  it('lays the value strip out as an even grid instead of ragged wrapping', () => {
    const css = read('src/styles/story-pages.css');
    const phone = /@media \(max-width: 760px\) \{[\s\S]*?\n\}/.exec(css)?.[0] ?? '';
    expect(phone).toMatch(/\.story-strip \{[^}]*display: grid;[^}]*grid-template-columns: repeat\(2, minmax\(0, 1fr\)\)/);
  });

  it('styles requirement chips with theme tokens so both themes work', () => {
    const css = read('src/styles/story-pages.css');
    const chip = /\.story-needs li \{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(chip).toContain('var(--border-strong)');
    expect(chip).toContain('var(--bg-elevated)');
    expect(chip).toContain('var(--fg1)');
    expect(chip).not.toMatch(/#[0-9a-f]{3,8}\b/i);
  });
});
