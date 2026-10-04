// Generates 1200x630 page cards and a square 512x512 service thumbnail as PNGs.
// Pipeline: satori (HTML/JSX-like tree -> SVG) + @resvg/resvg-js (SVG -> PNG).
// Landscape cards are reused per card when their PNG already exists; the small
// service mark is refreshed. Pass --force to regenerate every card.

import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import satori from 'satori';
import { Resvg } from '@resvg/resvg-js';
import sharp from 'sharp';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const ROOT = resolve(__dirname, '..');
// These licensed, source-controlled inputs keep card rendering independent of
// network availability and mutable upstream font URLs. See fonts/README.md.
const FONT_DIR = join(__dirname, 'fonts');
const OUT_DIR = join(ROOT, 'public', 'og');
const HERO_SRC = join(ROOT, 'public', 'illustrations', '01-homepage-hero-1600.webp');
const FORCE = process.argv.includes('--force');

const CARDS = [
  { slug: 'default',     title: 'Froglet', kicker: 'identity. execution. settlement.', tagline: 'Open protocol for AI agents. Trust through math, not middlemen.' },
  { slug: 'marketplace', title: 'Marketplace', kicker: 'Live providers. Indexed offers.', tagline: 'Registered providers, indexed offers, execution receipts, public deal feed.' },
  { slug: 'open-source', title: 'Open source', kicker: 'Protocol kernel. Reference impl.', tagline: 'The Froglet protocol kernel, reference implementation, and integrations are open source.' },
  { slug: 'learn',       title: 'Docs', kicker: 'Concepts. Quickstart. Spec.', tagline: 'Identity, deal flow, payment rails, settlement — everything to ship a node.' },
];

const FONTS = [
  {
    file: 'Inter-Bold.ttf',
    name: 'Inter',
    weight: 700,
    style: 'normal',
  },
  {
    file: 'Inter-Regular.ttf',
    name: 'Inter',
    weight: 400,
    style: 'normal',
  },
  {
    file: 'JetBrainsMono-Bold.ttf',
    name: 'JetBrains Mono',
    weight: 700,
    style: 'normal',
  },
];

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

// Art cards mirror a page hero: mono headline, green last line, and the page's illustration.
// The card colour is sampled from the art's own background so the art's edge is invisible.
const ART_CARDS = [
  {
    slug: 'home',
    art: HERO_SRC,
    artBox: { left: 400, top: 90, width: 800, height: 450 },
    fontSize: 66,
    textWidth: 590,
    kicker: 'Useful work deserves to be used.',
    lines: ['Share data', 'and tools.'],
    accent: 'On your terms.',
    tagline: 'Choose what to share, review it with your AI assistant, and send a link.',
    status: 'Public beta',
  },
  {
    slug: 'organizations',
    art: join(ROOT, 'public', 'illustrations', '10-open-source-1536.webp'),
    artBox: { left: 612, top: 116, width: 576, height: 384 },
    fontSize: 44,
    textWidth: 540,
    kicker: 'For organizations',
    lines: ['Share one service.', 'Set the limits.'],
    accent: 'Keep the record.',
    tagline: 'An open protocol in public beta. Access limits you set, signed records you can check.',
    status: 'Public beta',
  },
];

async function artCardAssets(card) {
  const { width, height } = card.artBox;
  const png = await sharp(card.art).resize({ width, height }).png().toBuffer();
  // Average the four corner blocks: a single pixel of the art's grain can be darker than its background.
  const { data, info } = await sharp(png).removeAlpha().raw().toBuffer({ resolveWithObject: true });
  const block = 16;
  const sum = [0, 0, 0];
  let n = 0;
  for (const [x0, y0] of [[0, 0], [info.width - block, 0], [0, info.height - block], [info.width - block, info.height - block]]) {
    for (let y = y0; y < y0 + block; y++) {
      for (let x = x0; x < x0 + block; x++) {
        for (let c = 0; c < 3; c++) sum[c] += data[(y * info.width + x) * info.channels + c];
        n++;
      }
    }
  }
  const bg = `#${sum.map((v) => Math.round(v / n).toString(16).padStart(2, '0')).join('')}`;
  return { uri: `data:image/png;base64,${png.toString('base64')}`, bg };
}

function artCardTree(card, { uri, bg }) {
  const mono = 'JetBrains Mono';
  const line = (text, color) => ({
    type: 'div',
    props: {
      style: { display: 'flex', fontFamily: mono, fontSize: `${card.fontSize}px`, fontWeight: 700, lineHeight: 1.12, letterSpacing: '-0.02em', color },
      children: text,
    },
  });
  return {
    type: 'div',
    props: {
      style: { position: 'relative', width: '1200px', height: '630px', display: 'flex', backgroundColor: bg, fontFamily: 'Inter' },
      children: [
        { type: 'img', props: { src: uri, width: card.artBox.width, height: card.artBox.height, style: { position: 'absolute', left: `${card.artBox.left}px`, top: `${card.artBox.top}px` } } },
        {
          type: 'div',
          props: {
            style: { position: 'absolute', left: '72px', top: '68px', width: `${card.textWidth}px`, height: '494px', display: 'flex', flexDirection: 'column', justifyContent: 'space-between' },
            children: [
              {
                type: 'div',
                props: {
                  style: { display: 'flex', flexDirection: 'column' },
                  children: [
                    { type: 'div', props: { style: { display: 'flex', fontFamily: mono, fontSize: '22px', fontWeight: 700, letterSpacing: '0.1em', textTransform: 'uppercase', color: '#52c72a', marginBottom: '26px' }, children: card.kicker } },
                    ...card.lines.map((t) => line(t, '#e8ede6')),
                    line(card.accent, '#52c72a'),
                    { type: 'div', props: { style: { display: 'flex', fontSize: '26px', fontWeight: 400, lineHeight: 1.45, color: '#9aa497', marginTop: '28px' }, children: card.tagline } },
                  ],
                },
              },
              {
                type: 'div',
                props: {
                  style: { display: 'flex', alignItems: 'center', fontFamily: mono, fontSize: '30px', fontWeight: 700 },
                  children: [
                    { type: 'div', props: { style: { display: 'flex', color: '#52c72a' }, children: '_o..o_' } },
                    { type: 'div', props: { style: { display: 'flex', color: '#e8ede6', marginLeft: '16px' }, children: 'froglet' } },
                    { type: 'div', props: { style: { display: 'flex', color: '#9aa497', marginLeft: '20px', fontSize: '22px' }, children: `· ${card.status}` } },
                  ],
                },
              },
            ],
          },
        },
      ],
    },
  };
}

/** A card is (re)generated only when its PNG is missing, unless --force is given. */
function needsRender(slug) {
  return FORCE || !existsSync(join(OUT_DIR, `${slug}.png`));
}

async function generateServiceThumbnail() {
  // Use the existing mark; this small, static asset needs no provider request.
  const mark = await readFile(join(ROOT, 'public', 'logo-mark.svg'), 'utf8');
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512"><rect width="512" height="512" rx="96" fill="#101417"/>${mark.replace('<svg ', '<svg x="40" y="184" width="432" height="144" ').replace('currentColor', '#52c72a')}</svg>`;
  const png = new Resvg(svg, { font: { fontFiles: [join(FONT_DIR, 'JetBrainsMono-Bold.ttf')], defaultFontFamily: 'JetBrains Mono', loadSystemFonts: false } }).render().asPng();
  await writeFile(join(OUT_DIR, 'service.png'), png);
  console.log('[og] wrote square service thumbnail');
}

async function main() {
  await mkdir(OUT_DIR, { recursive: true });

  await generateServiceThumbnail();

  const pendingCards = CARDS.filter((card) => needsRender(card.slug));
  const pendingArt = ART_CARDS.filter((card) => needsRender(card.slug));
  if (pendingCards.length === 0 && pendingArt.length === 0) {
    console.log('[og] up to date');
    return;
  }

  const fontData = await Promise.all(FONTS.map((font) => readFile(join(FONT_DIR, font.file))));
  const fontConfig = FONTS.map((f, i) => ({
    name: f.name,
    data: fontData[i],
    weight: f.weight,
    style: f.style,
  }));

  async function render(slug, tree) {
    const svg = await satori(tree, { width: 1200, height: 630, fonts: fontConfig });
    const png = new Resvg(svg, { fitTo: { mode: 'width', value: 1200 } })
      .render()
      .asPng();
    const out = join(OUT_DIR, `${slug}.png`);
    await writeFile(out, png);
    console.log(`[og] wrote ${out} (${(png.length / 1024).toFixed(1)} KB)`);
  }

  for (const card of pendingCards) {
    await render(card.slug, cardTree(card));
  }
  for (const card of pendingArt) {
    await render(card.slug, artCardTree(card, await artCardAssets(card)));
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
