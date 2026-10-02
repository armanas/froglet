// Shared by the page tests: resolve an internal route or anchor to the file that defines it.
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const src = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
export const docsSite = resolve(src, '..');
export const repoRoot = resolve(docsSite, '..');

/** The same slug rule Starlight's headings use: lowercase, punctuation dropped, spaces to hyphens. */
export const slug = (heading: string) => heading.trim().toLowerCase().replace(/[^\w\s-]/g, '').replace(/\s+/g, '-');

/** Where an internal route is defined: an Astro page, a docs entry, or a public file. */
export function routeSource(pathname: string): string | undefined {
  const trimmed = pathname.replace(/^\/|\/$/g, '');
  const candidates = [
    `pages/${trimmed}.astro`,
    `pages/${trimmed}/index.astro`,
    `content/docs/${trimmed}.md`,
    `content/docs/${trimmed}.mdx`,
    `content/docs/${trimmed}/index.md`,
    `content/docs/${trimmed}/index.mdx`,
  ];
  const found = candidates.find((candidate) => existsSync(resolve(src, candidate)));
  if (found) return resolve(src, found);
  const publicFile = resolve(docsSite, 'public', trimmed);
  return trimmed && existsSync(publicFile) ? publicFile : undefined;
}

export function hasAnchor(file: string, anchor: string): boolean {
  const text = readFileSync(file, 'utf8');
  if (text.includes(`id="${anchor}"`) || text.includes(`id='${anchor}'`)) return true;
  return text.split('\n').some((line) => /^#{1,6}\s/.test(line) && slug(line.replace(/^#{1,6}\s+/, '')) === anchor);
}

/** Internal links (`href="/x/#y"` and `href: '/x/'`) that do not resolve to a page or an anchor. */
export function brokenInternalLinks(source: string): string[] {
  const links = new Set([
    ...Array.from(source.matchAll(/href="(\/[^"#?]*)(#[^"]*)?"/g)).map((match) => `${match[1]}${match[2] ?? ''}`),
    ...Array.from(source.matchAll(/href: '(\/[^'#?]*)(#[^']*)?'/g)).map((match) => `${match[1]}${match[2] ?? ''}`),
  ]);
  const problems: string[] = [];
  for (const link of links) {
    const [pathname, hash] = link.split('#');
    const file = routeSource(pathname);
    if (!file) problems.push(`${link}: no page`);
    else if (hash && !hasAnchor(file, hash)) problems.push(`${link}: no #${hash}`);
  }
  return problems;
}
