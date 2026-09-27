import { describe, expect, it } from 'vitest';
import sharp from 'sharp';
import jsQR from 'jsqr';
import { serviceQrSvg } from '../../data/service-qr';

describe('service QR images', () => {
  it.each(['hla-peptidome-catalog', 'a'.repeat(128)])('decodes the exact canonical link for %s', async service => {
    const url = `https://froglet.dev/s/${'ab'.repeat(32)}/${service}`;
    const {data, info} = await sharp(Buffer.from(serviceQrSvg(url))).resize(600,600,{kernel:'nearest'}).ensureAlpha().raw().toBuffer({resolveWithObject:true});
    const decoded = jsQR(new Uint8ClampedArray(data), info.width, info.height);
    expect(decoded?.data).toBe(url);
  });
});
