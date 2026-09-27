import { describe, expect, it } from 'vitest';
import { createHash } from 'node:crypto';
import sharp from 'sharp';
import jsQR from 'jsqr';
import { serviceQrSvg } from '../../data/service-qr';

const samples = [1, 8, 24, 32, 48, 64, 96, 128].flatMap(length =>
  ['https://froglet.dev', 'https://candidate.froglet.dev'].map(origin => {
    const provider = createHash('sha256').update(`${origin}:${length}`).digest('hex');
    const service = 'Catalog_9.test-'.repeat(10).slice(0, length);
    return `${origin}/s/${provider}/${service}`;
  }));

describe('service QR images', () => {
  it.each(samples)('decodes the branded image for %s at display and print sizes', async url => {
    const svg = serviceQrSvg(url);
    expect(svg).toContain('Froglet logo');
    expect(svg).toContain('_o..o_');
    expect(svg).not.toMatch(/<image|href=|<script/);
    for (const size of [280, 360, 600]) {
      const nativeSize = Number(svg.match(/viewBox="0 0 (\d+)/)?.[1]);
      const { data, info } = await sharp(Buffer.from(svg), { density: size / nativeSize * 72 }).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
      // Decode the finished image, including the opaque logo overlay.
      expect(jsQR(new Uint8ClampedArray(data), info.width, info.height)?.data).toBe(url);
    }
  });
});
