import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { navLinks } from '../../data/nav-links';

const here = dirname(fileURLToPath(import.meta.url));
const siteRoot = resolve(here, '../../..');

function read(path: string): string {
  return readFileSync(resolve(siteRoot, path), 'utf8');
}

/** Width and height from a PNG's IHDR chunk (big-endian, bytes 16-23). */
function pngSize(path: string): { width: number; height: number } {
  const bytes = readFileSync(resolve(siteRoot, path));
  expect(bytes.subarray(1, 4).toString('ascii'), `${path} is a PNG`).toBe('PNG');
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

describe('homepage social preview', () => {
  it('uses a dedicated 1200x630 card, not the 3:2 WebP illustration', () => {
    const home = read('src/pages/index.astro');
    expect(home).toContain("src: 'https://froglet.dev/og/home.png'");
    expect(home).toContain('width: 1200');
    expect(home).toContain('height: 630');
    expect(home).toContain("type: 'image/png'");
    expect(pngSize('public/og/home.png')).toEqual({ width: 1200, height: 630 });
  });

  it('describes the image to link-preview consumers, with size and type', () => {
    const layout = read('src/layouts/StoryLayout.astro');
    for (const tag of [
      'property="og:image" content={image.src}',
      'property="og:image:type" content={image.type}',
      'property="og:image:width" content={String(image.width)}',
      'property="og:image:height" content={String(image.height)}',
      'property="og:image:alt" content={image.alt}',
      'name="twitter:image" content={image.src}',
      'name="twitter:image:alt" content={image.alt}',
    ]) {
      expect(layout, tag).toContain(tag);
    }
    // Every page that does not pass its own image keeps the old default, with correct metadata.
    expect(layout).toContain('05-walkthrough-share-1536.webp');
    expect(layout).toContain('width: 1536');
    expect(layout).toContain('height: 1024');
  });

  it('generates the card with the site generator, one card at a time', () => {
    const generator = read('scripts/generate-og.mjs');
    expect(generator).toContain("slug: 'home'");
    expect(generator).toContain("process.argv.includes('--force')");
    // Adding a card must not rewrite the other cards' tracked PNGs.
    expect(generator).toContain('CARDS.filter((card) => needsRender(card.slug))');
    expect(generator).toContain('ART_CARDS.filter((card) => needsRender(card.slug))');
  });
});

describe('crawler discovery', () => {
  it('serves a robots.txt that points at the sitemap and keeps noindex pages crawlable', () => {
    const robots = read('public/robots.txt');
    expect(robots).toContain('User-agent: *');
    expect(robots).toContain('Allow: /');
    expect(robots).toContain('Disallow: /api/');
    expect(robots).toContain('Sitemap: https://froglet.dev/sitemap-index.xml');
    // Shared-service pages carry a noindex tag; blocking them would hide that tag from crawlers.
    expect(robots).not.toMatch(/Disallow:\s*\/s\//);
    expect(read('src/pages/service.astro')).toContain('name="robots" content="noindex"');
  });

  it('links the sitemap from story pages, as the docs pages already do', () => {
    expect(read('src/layouts/StoryLayout.astro')).toContain('<link rel="sitemap" href="/sitemap-index.xml" />');
  });

  it('keeps the missing-page response out of search results without claiming a canonical page', () => {
    const source = read('src/pages/404.astro');
    const head = /<head>([\s\S]*?)<\/head>/.exec(source)?.[1];
    expect(head).toBeDefined();
    const document = new DOMParser().parseFromString(`<html><head>${head}</head></html>`, 'text/html');
    const robots = document.querySelector('meta[name="robots"]')?.getAttribute('content')?.split(/[,\s]+/);
    expect(robots).toContain('noindex');
    expect(document.querySelector('link[rel="canonical"]')).toBeNull();
    expect(document.querySelector('meta[property="og:url"]')).toBeNull();
  });
});

describe('homepage calls to action', () => {
  it('labels every in-page link to the publish page with that page\'s own title', () => {
    const home = read('src/pages/index.astro');
    const publishTitle = /title="([^"]+?) · Froglet"/.exec(read('src/pages/publish.astro'))?.[1];
    expect(publishTitle).toBe('Share something useful');

    const labels = [...home.matchAll(/<a [^>]*href="\/publish\/"[^>]*>([^<]*)<\/a>/g)].map((match) => match[1]);
    expect(labels.length).toBeGreaterThanOrEqual(3);
    expect(new Set(labels)).toEqual(new Set([publishTitle]));
  });

  it('keeps the short section name for navigation, which is a different job from a button', () => {
    expect(navLinks.find((item) => item.href === '/publish/')?.label).toBe('Start sharing');
    expect(read('src/components/SiteFooter.astro')).toContain('href="/publish/">Start sharing</a>');
  });
});
