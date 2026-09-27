import qrcode from 'qrcode-generator';

/** Generate locally; the link is never sent to a third-party QR service. */
export function serviceQrSvg(canonicalUrl: string): string {
  const qr = qrcode(0, 'M');
  qr.addData(canonicalUrl, 'Byte');
  qr.make();
  // Four-module quiet zone, opaque white background, crisp integer modules.
  return qr.createSvgTag({ cellSize: 6, margin: 24, scalable: true, alt: 'QR code for the Froglet service link' });
}
