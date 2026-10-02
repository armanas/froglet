import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const siteRoot = resolve(here, '../../..');

function read(path: string): string {
  return readFileSync(resolve(siteRoot, path), 'utf8');
}

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) return name === '__tests__' ? [] : sourceFiles(full);
    return /\.(astro|css|ts|mdx?|mjs)$/.test(name) ? [full] : [];
  });
}

describe('self-hosted fonts', () => {
  it('never requests fonts from a third-party host', () => {
    const files = [...sourceFiles(resolve(siteRoot, 'src')), resolve(siteRoot, 'astro.config.mjs')];
    const offenders = files.filter((file) => /fonts\.(googleapis|gstatic)\.com/.test(readFileSync(file, 'utf8')));
    expect(offenders).toEqual([]);
  });

  it('loads Inter and JetBrains Mono from the bundled fontsource packages', () => {
    const tokens = read('src/styles/tokens.css');
    expect(tokens).toContain("@import '@fontsource-variable/inter/wght.css';");
    expect(tokens).toContain("@import '@fontsource-variable/jetbrains-mono/wght.css';");
    // The variable family name comes first; the plain name stays as a local-install fallback.
    expect(tokens).toMatch(/--font-sans:\s*'Inter Variable', 'Inter',/);
    expect(tokens).toMatch(/--font-mono:\s*'JetBrains Mono Variable', 'JetBrains Mono',/);
    const pkg = JSON.parse(read('package.json')) as { dependencies: Record<string, string> };
    expect(pkg.dependencies['@fontsource-variable/inter']).toMatch(/^\d/);
    expect(pkg.dependencies['@fontsource-variable/jetbrains-mono']).toMatch(/^\d/);
  });

  it('preloads the two latin variable fonts with the attributes browsers require', () => {
    const preload = read('src/components/FontPreload.astro');
    expect(preload).toContain('inter-latin-wght-normal.woff2?url');
    expect(preload).toContain('jetbrains-mono-latin-wght-normal.woff2?url');
    expect(preload.match(/<link rel="preload"[^>]*as="font" type="font\/woff2" crossorigin/g)).toHaveLength(2);
  });

  it('preloads on every page head that loads the theme, and on the docs head', () => {
    const pages = ['src/layouts/StoryLayout.astro', ...readdirSync(resolve(siteRoot, 'src/pages')).filter((n) => n.endsWith('.astro')).map((n) => `src/pages/${n}`)];
    for (const page of pages) {
      const source = read(page);
      if (source.includes('<ThemeProvider />')) {
        expect(source, `${page} loads the theme but not the font preload`).toContain('<FontPreload />');
      }
    }
    expect(read('src/components/StarlightHead.astro')).toContain('<FontPreload />');
  });

  it('names the variable families in every canvas font string', () => {
    for (const file of sourceFiles(resolve(siteRoot, 'src/scripts'))) {
      const source = readFileSync(file, 'utf8');
      const strings = source.match(/[^\n]*(JetBrains Mono|"Inter"|'Inter')[^\n]*/g) ?? [];
      for (const line of strings) {
        if (/JetBrains Mono(?! Variable)/.test(line)) expect(line, `${file}`).toContain('JetBrains Mono Variable');
        if (/["']Inter["']/.test(line)) expect(line, `${file}`).toContain('Inter Variable');
      }
    }
  });

  it('caches hashed build assets for a year, replacing the cache Google Fonts used to provide', () => {
    expect(read('public/_headers')).toMatch(/\/_astro\/\*\n\s+Cache-Control: public, max-age=31536000, immutable/);
  });

  it('ships the SIL OFL licence text for both bundled fonts', () => {
    for (const file of ['public/licenses/Inter-OFL-1.1.txt', 'public/licenses/JetBrainsMono-OFL-1.1.txt']) {
      expect(existsSync(resolve(siteRoot, file)), file).toBe(true);
      expect(read(file)).toContain('SIL OPEN FONT LICENSE Version 1.1');
    }
  });
});
