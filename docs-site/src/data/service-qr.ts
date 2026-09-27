import qrcode from 'qrcode-generator';

/** Generate locally; the link is never sent to a third-party QR service. */
export function serviceQrSvg(canonicalUrl: string): string {
  // The central badge obscures data modules: use the highest error correction.
  const qr = qrcode(0, 'H');
  qr.addData(canonicalUrl, 'Byte');
  qr.make();
  const cell = 6;
  const margin = 4 * cell;
  const size = qr.getModuleCount() * cell + 2 * margin;
  // Keep the wide, shallow _o..o_ mark under 4% of the code's area.
  // Inline it so downloaded SVGs need neither the website nor an external font.
  const width = Math.floor(qr.getModuleCount() * 0.23) * cell;
  const height = 5 * cell;
  const x = (size - width) / 2;
  const y = (size - height) / 2;
  const badge = `<g aria-label="Froglet logo"><rect x="${x}" y="${y}" width="${width}" height="${height}" rx="4" fill="#fff"/><text x="${size / 2}" y="${size / 2 + 4}" text-anchor="middle" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" font-weight="700" font-size="${height * 0.7}" textLength="${width - 12}" lengthAdjust="spacingAndGlyphs" fill="#287d18">_o..o_</text></g>`;
  // Four-module quiet zone, opaque white background, crisp integer modules.
  return qr.createSvgTag({ cellSize: cell, margin, scalable: true, alt: 'QR code for the Froglet service link' }).replace('<path ', '<path shape-rendering="crispEdges" ').replace('</svg>', `${badge}</svg>`);
}
