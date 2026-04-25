// Generates 1200x630 OG/Twitter social cards as PNGs.
// Pipeline: satori (HTML/JSX-like tree -> SVG) + @resvg/resvg-js (SVG -> PNG).
// Idempotent: skips work when all PNGs and cached fonts exist.

import { mkdir, readFile, writeFile, stat } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import satori from 'satori';
import { Resvg } from '@resvg/resvg-js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const ROOT = resolve(__dirname, '..');
const FONT_DIR = join(__dirname, '.fonts');
const OUT_DIR = join(ROOT, 'public', 'og');

const CARDS = [
  { slug: 'default',     title: 'Froglet', kicker: 'identity. execution. settlement.', tagline: 'Open protocol for AI agents. Trust through math, not middlemen.' },
  { slug: 'marketplace', title: 'Marketplace', kicker: 'Live providers. Indexed offers.', tagline: 'Registered providers, indexed offers, execution receipts, public deal feed.' },
  { slug: 'managed',     title: 'Managed', kicker: 'Cloud-hosted Froglet.', tagline: 'Run nodes without running infrastructure. Coming soon.' },
  { slug: 'open-source', title: 'Open source', kicker: 'Protocol kernel. Reference impl.', tagline: 'The Froglet protocol kernel, reference implementation, and integrations are open source.' },
  { slug: 'learn',       title: 'Docs', kicker: 'Concepts. Quickstart. Spec.', tagline: 'Identity, deal flow, payment rails, settlement — everything to ship a node.' },
];

const FONTS = [
  {
    file: 'Inter-Bold.ttf',
    sources: [
      'https://cdn.jsdelivr.net/fontsource/fonts/inter@latest/latin-700-normal.ttf',
      'https://raw.githubusercontent.com/rsms/inter/master/docs/font-files/InterDisplay-Bold.otf',
    ],
    name: 'Inter',
    weight: 700,
    style: 'normal',
  },
  {
    file: 'Inter-Regular.ttf',
    sources: [
      'https://cdn.jsdelivr.net/fontsource/fonts/inter@latest/latin-400-normal.ttf',
      'https://raw.githubusercontent.com/rsms/inter/master/docs/font-files/InterDisplay-Regular.otf',
    ],
    name: 'Inter',
    weight: 400,
    style: 'normal',
  },
  {
    file: 'JetBrainsMono-Bold.ttf',
    sources: [
      'https://cdn.jsdelivr.net/fontsource/fonts/jetbrains-mono@latest/latin-700-normal.ttf',
      'https://raw.githubusercontent.com/JetBrains/JetBrainsMono/master/fonts/ttf/JetBrainsMono-Bold.ttf',
    ],
    name: 'JetBrains Mono',
    weight: 700,
    style: 'normal',
  },
];

async function fetchFont(spec) {
  const target = join(FONT_DIR, spec.file);
  if (existsSync(target)) {
    const s = await stat(target);
    if (s.size > 1024) return target;
  }
  const errors = [];
  for (const url of spec.sources) {
    try {
      const res = await fetch(url);
      if (!res.ok) {
        errors.push(`${url} -> HTTP ${res.status}`);
        continue;
      }
      const buf = Buffer.from(await res.arrayBuffer());
      if (buf.length < 1024) {
        errors.push(`${url} -> too small (${buf.length} bytes)`);
        continue;
      }
      await writeFile(target, buf);
      return target;
    } catch (e) {
      errors.push(`${url} -> ${e.message}`);
    }
  }
  throw new Error(`[og] failed to fetch font ${spec.file}\n  ${errors.join('\n  ')}`);
}

function cardTree({ title, kicker, tagline }) {
  return {
    type: 'div',
    props: {
      style: {
        width: '1200px',
        height: '630px',
        display: 'flex',
        flexDirection: 'row',
        backgroundColor: '#0a0d0a',
        fontFamily: 'Inter',
      },
      children: [
        {
          type: 'div',
          props: {
            style: {
              width: '8px',
              height: '630px',
              backgroundColor: '#f5c518',
              display: 'flex',
            },
          },
        },
        {
          type: 'div',
          props: {
            style: {
              flex: 1,
              height: '630px',
              padding: '80px',
              display: 'flex',
              flexDirection: 'column',
              justifyContent: 'space-between',
            },
            children: [
              {
                type: 'div',
                props: {
                  style: {
                    display: 'flex',
                    flexDirection: 'column',
                  },
                  children: [
                    {
                      type: 'div',
                      props: {
                        style: {
                          fontSize: '40px',
                          fontWeight: 700,
                          color: '#9aa497',
                          marginBottom: '24px',
                          letterSpacing: '-0.01em',
                          display: 'flex',
                        },
                        children: kicker,
                      },
                    },
                    {
                      type: 'div',
                      props: {
                        style: {
                          fontSize: '96px',
                          fontWeight: 700,
                          color: '#f5f5f5',
                          letterSpacing: '-0.03em',
                          lineHeight: 1.05,
                          marginBottom: '32px',
                          display: 'flex',
                        },
                        children: title,
                      },
                    },
                    {
                      type: 'div',
                      props: {
                        style: {
                          fontSize: '28px',
                          fontWeight: 400,
                          color: '#e8ede6',
                          lineHeight: 1.4,
                          maxWidth: '960px',
                          display: 'flex',
                        },
                        children: tagline,
                      },
                    },
                  ],
                },
              },
              {
                type: 'div',
                props: {
                  style: {
                    fontFamily: 'JetBrains Mono',
                    fontSize: '36px',
                    fontWeight: 700,
                    color: '#52c72a',
                    display: 'flex',
                  },
                  children: '_o..o_',
                },
              },
            ],
          },
        },
      ],
    },
  };
}

async function allOutputsExist() {
  for (const c of CARDS) {
    if (!existsSync(join(OUT_DIR, `${c.slug}.png`))) return false;
  }
  for (const f of FONTS) {
    if (!existsSync(join(FONT_DIR, f.file))) return false;
  }
  return true;
}

async function main() {
  await mkdir(FONT_DIR, { recursive: true });
  await mkdir(OUT_DIR, { recursive: true });

  if (await allOutputsExist()) {
    console.log('[og] up to date');
    return;
  }

  const fontPaths = await Promise.all(FONTS.map(fetchFont));
  const fontData = await Promise.all(fontPaths.map((p) => readFile(p)));
  const fontConfig = FONTS.map((f, i) => ({
    name: f.name,
    data: fontData[i],
    weight: f.weight,
    style: f.style,
  }));

  for (const card of CARDS) {
    const svg = await satori(cardTree(card), {
      width: 1200,
      height: 630,
      fonts: fontConfig,
    });
    const png = new Resvg(svg, { fitTo: { mode: 'width', value: 1200 } })
      .render()
      .asPng();
    const out = join(OUT_DIR, `${card.slug}.png`);
    await writeFile(out, png);
    console.log(`[og] wrote ${out} (${(png.length / 1024).toFixed(1)} KB)`);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
