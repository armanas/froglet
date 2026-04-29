/**
 * Identity diagram canvas renderer.
 * Shows keypair generation and the relationship between
 * private key, public key, and node identity.
 */

export interface IdentityElement {
  label: string;
  sub: string;
  color: string;
  detail: string;
}

export const ELEMENTS: IdentityElement[] = [
  {
    label: 'Private Key',
    sub: '256-bit secret',
    color: '#e54848',
    detail: 'A random 256-bit number (32 bytes). Never shared. Used to sign artifacts with BIP340 Schnorr signatures.',
  },
  {
    label: 'Public Key',
    sub: 'curve point',
    color: '#4ea3ff',
    detail: 'Computed as P = k · G on the secp256k1 curve. Cannot be reversed to find the private key. Used to verify signatures.',
  },
  {
    label: 'Node ID',
    sub: '64-char hex',
    color: '#52c72a',
    detail: 'The 32-byte x-only public key encoded as a 64-character lowercase hex string. This is your identity on the network.',
  },
];

const OPERATIONS = ['secp256k1 · G', 'x-only hex encode'];

const LAYOUT = {
  padding: { top: 28, right: 30, bottom: 28, left: 30 },
  box: { width: 158, height: 62, radius: 12 },
  arrowHead: 7,
  fonts: {
    label: '700 14px "JetBrains Mono", ui-monospace, monospace',
    sub: '500 11px "Inter", system-ui, sans-serif',
    pill: '700 10px "JetBrains Mono", ui-monospace, monospace',
  },
} as const;

interface Palette {
  surface: string;
  surface2: string;
  border: string;
  borderStrong: string;
  fg1: string;
  fg2: string;
  fg3: string;
}

function cssVar(name: string, fallback: string): string {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value || fallback;
}

function readPalette(): Palette {
  return {
    surface: cssVar('--bg-elevated', '#171b18'),
    surface2: cssVar('--bg-elevated-2', '#1c201d'),
    border: cssVar('--border', '#1f2320'),
    borderStrong: cssVar('--border-strong', '#2b3028'),
    fg1: cssVar('--fg1', '#e8ede6'),
    fg2: cssVar('--fg2', '#9aa497'),
    fg3: cssVar('--fg3', '#6b7568'),
  };
}

function resizeCanvas(canvas: HTMLCanvasElement, ctx: CanvasRenderingContext2D): { w: number; h: number } {
  const rect = canvas.getBoundingClientRect();
  const w = Math.max(320, Math.round(rect.width || Number(canvas.getAttribute('width')) || 700));
  const h = Math.max(130, Math.round(rect.height || Number(canvas.getAttribute('height')) || 150));
  const dpr = Math.max(1, Math.min(window.devicePixelRatio || 1, 2));
  const targetW = Math.round(w * dpr);
  const targetH = Math.round(h * dpr);
  if (canvas.width !== targetW || canvas.height !== targetH) {
    canvas.width = targetW;
    canvas.height = targetH;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { w, h };
}

function roundedRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number): void {
  const rr = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + rr, y);
  ctx.lineTo(x + w - rr, y);
  ctx.quadraticCurveTo(x + w, y, x + w, y + rr);
  ctx.lineTo(x + w, y + h - rr);
  ctx.quadraticCurveTo(x + w, y + h, x + w - rr, y + h);
  ctx.lineTo(x + rr, y + h);
  ctx.quadraticCurveTo(x, y + h, x, y + h - rr);
  ctx.lineTo(x, y + rr);
  ctx.quadraticCurveTo(x, y, x + rr, y);
  ctx.closePath();
}

function layoutBoxes(w: number, h: number): Array<{ cx: number; cy: number; bw: number; bh: number }> {
  const usableW = w - LAYOUT.padding.left - LAYOUT.padding.right;
  const n = ELEMENTS.length;
  const bw = Math.min(LAYOUT.box.width, Math.max(96, (usableW - 28 * (n - 1)) / n));
  const bh = LAYOUT.box.height;
  const gap = n > 1 ? Math.max(20, (usableW - n * bw) / (n - 1)) : 0;
  const cy = h / 2 + 10;
  return ELEMENTS.map((_, i) => ({
    cx: LAYOUT.padding.left + bw / 2 + i * (bw + gap),
    cy,
    bw,
    bh,
  }));
}

function drawIdentityDiagram(
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  selected: number,
  hovered: number,
  palette: Palette,
): void {
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = palette.surface;
  ctx.fillRect(0, 0, w, h);

  const boxes = layoutBoxes(w, h);

  for (let i = 0; i < boxes.length - 1; i++) {
    const from = boxes[i];
    const to = boxes[i + 1];
    const x1 = from.cx + from.bw / 2 + 14;
    const x2 = to.cx - to.bw / 2 - 14;
    const midX = (x1 + x2) / 2;
    const cy = from.cy;

    ctx.strokeStyle = palette.borderStrong;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(x1, cy);
    ctx.lineTo(x2, cy);
    ctx.stroke();

    ctx.beginPath();
    ctx.moveTo(x2 - LAYOUT.arrowHead, cy - LAYOUT.arrowHead);
    ctx.lineTo(x2, cy);
    ctx.lineTo(x2 - LAYOUT.arrowHead, cy + LAYOUT.arrowHead);
    ctx.stroke();

    const pillText = OPERATIONS[i];
    ctx.font = LAYOUT.fonts.pill;
    const pillW = ctx.measureText(pillText).width + 22;
    const pillH = 26;
    const pillX = midX - pillW / 2;
    const pillY = cy - 54;
    roundedRect(ctx, pillX, pillY, pillW, pillH, 13);
    ctx.fillStyle = palette.surface2;
    ctx.fill();
    ctx.strokeStyle = palette.border;
    ctx.stroke();
    ctx.fillStyle = palette.fg2;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText(pillText, midX, pillY + pillH / 2 + 0.5);
  }

  for (let i = 0; i < boxes.length; i++) {
    const el = ELEMENTS[i];
    const box = boxes[i];
    const isSelected = i === selected;
    const isHovered = i === hovered && !isSelected;
    const x = box.cx - box.bw / 2;
    const y = box.cy - box.bh / 2;

    if (isSelected) {
      const glow = ctx.createRadialGradient(box.cx, box.cy, box.bh / 2, box.cx, box.cy, box.bh * 1.9);
      glow.addColorStop(0, `${el.color}3d`);
      glow.addColorStop(1, `${el.color}00`);
      ctx.fillStyle = glow;
      ctx.beginPath();
      ctx.arc(box.cx, box.cy, box.bh * 1.9, 0, Math.PI * 2);
      ctx.fill();
    }

    roundedRect(ctx, x, y, box.bw, box.bh, LAYOUT.box.radius);
    ctx.fillStyle = isSelected ? `${el.color}22` : palette.surface2;
    ctx.fill();
    ctx.strokeStyle = isSelected || isHovered ? el.color : palette.borderStrong;
    ctx.lineWidth = isSelected ? 2.5 : 1.5;
    ctx.stroke();

    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.font = LAYOUT.fonts.label;
    ctx.fillStyle = isSelected || isHovered ? el.color : palette.fg1;
    ctx.fillText(el.label, box.cx, box.cy - 8);
    ctx.font = LAYOUT.fonts.sub;
    ctx.fillStyle = palette.fg2;
    ctx.fillText(el.sub, box.cx, box.cy + 13);
  }

  ctx.textBaseline = 'alphabetic';
}

function renderDetail(detailEl: HTMLElement, index: number): void {
  const el = ELEMENTS[index];
  detailEl.innerHTML =
    `<strong style="color:${el.color}">${el.label}</strong> ` +
    `<span class="plot-muted">(${el.sub})</span><br>` +
    `${el.detail}`;
}

function hitIndex(x: number, w: number, h: number): number {
  const boxes = layoutBoxes(w, h);
  return boxes.findIndex((box) => x >= box.cx - box.bw / 2 && x <= box.cx + box.bw / 2);
}

export function initIdentityDiagram(canvas: HTMLCanvasElement, detailEl: HTMLElement): void {
  const ctx = canvas.getContext('2d');
  if (!ctx) {
    console.warn('[identity-diagram] canvas context unavailable');
    return;
  }

  let selected = 0;
  let hovered = -1;
  let viewport = { w: Number(canvas.getAttribute('width')) || 700, h: Number(canvas.getAttribute('height')) || 150 };

  function safeDraw(): void {
    try {
      viewport = resizeCanvas(canvas, ctx!);
      drawIdentityDiagram(ctx!, viewport.w, viewport.h, selected, hovered, readPalette());
    } catch (err) {
      console.error('[identity-diagram] draw error:', err);
    }
  }

  function selectElement(index: number): void {
    if (index < 0 || index >= ELEMENTS.length || index === selected) return;
    selected = index;
    safeDraw();
    renderDetail(detailEl, index);
  }

  canvas.addEventListener('click', (e: MouseEvent) => {
    const rect = canvas.getBoundingClientRect();
    selectElement(hitIndex(e.clientX - rect.left, viewport.w, viewport.h));
  });

  canvas.addEventListener('mousemove', (e: MouseEvent) => {
    const rect = canvas.getBoundingClientRect();
    const next = hitIndex(e.clientX - rect.left, viewport.w, viewport.h);
    if (next !== hovered) {
      hovered = next;
      safeDraw();
    }
  });

  canvas.addEventListener('mouseleave', () => {
    hovered = -1;
    safeDraw();
  });

  window.addEventListener('resize', safeDraw, { passive: true });
  safeDraw();
  renderDetail(detailEl, 0);
}
