/**
 * Identity diagram canvas renderer.
 * Shows keypair generation and the relationship between
 * private key, public key, and node identity.
 *
 * Flow: Private Key → (secp256k1 multiplication) → Public Key → (truncate/hex) → Node ID
 *
 * Validates: Requirements 22.1, 22.6
 */

// ---------------------------------------------------------------------------
// Layout constants (no magic numbers)
// ---------------------------------------------------------------------------

export const IDENTITY_LAYOUT = {
  /** Padding around the canvas edges */
  PADDING: { top: 20, right: 20, bottom: 20, left: 20 },
  /** Box dimensions for each element */
  BOX: {
    WIDTH: 130,
    HEIGHT: 52,
    RADIUS: 10,
  },
  /** Operation label pill dimensions */
  PILL: {
    HEIGHT: 24,
    RADIUS: 12,
    FONT: '10px system-ui, sans-serif',
  },
  /** Arrow connector settings */
  ARROW: {
    HEAD_SIZE: 5,
    LINE_WIDTH: 1.5,
    COLOR: '#30363d',
  },
  /** Highlight glow settings */
  GLOW: {
    RADIUS_MULTIPLIER: 1.6,
    ALPHA: 0x28,
  },
  /** Fonts */
  FONTS: {
    label: 'bold 12px system-ui, sans-serif',
    sub: '10px system-ui, sans-serif',
    pill: '9px system-ui, sans-serif',
  },
  /** Colors for each element */
  COLORS: {
    privateKey: '#f87171',
    publicKey: '#a78bfa',
    nodeId: '#4ade80',
    pillBg: '#1a2332',
    pillText: '#9ca3af',
    subText: '#6b7280',
    selectedStroke: 2,
    defaultStroke: 1.2,
  },
} as const;

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

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
    color: IDENTITY_LAYOUT.COLORS.privateKey,
    detail: 'A random 256-bit number (32 bytes). Never shared. Used to sign artifacts with BIP340 Schnorr signatures.',
  },
  {
    label: 'Public Key',
    sub: 'curve point',
    color: IDENTITY_LAYOUT.COLORS.publicKey,
    detail: 'Computed as P = k · G on the secp256k1 curve. Cannot be reversed to find the private key. Used to verify signatures.',
  },
  {
    label: 'Node ID',
    sub: '64-char hex',
    color: IDENTITY_LAYOUT.COLORS.nodeId,
    detail: 'The 32-byte x-only public key encoded as a 64-character lowercase hex string. This is your identity on the network.',
  },
];

export const OPERATIONS: string[] = [
  'secp256k1 · G',
  'x-only hex encode',
];

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

function drawIdentityDiagram(
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  selected: number,
  hovered: number,
): void {
  const L = IDENTITY_LAYOUT;
  const n = ELEMENTS.length;

  ctx.clearRect(0, 0, w, h);

  // Compute horizontal positions: evenly distribute boxes
  const usableW = w - L.PADDING.left - L.PADDING.right;
  const totalBoxW = n * L.BOX.WIDTH;
  const totalGap = usableW - totalBoxW;
  const gap = totalGap / (n - 1);
  const cy = h / 2;

  const boxPositions: { cx: number; cy: number }[] = [];
  for (let i = 0; i < n; i++) {
    const cx_i = L.PADDING.left + L.BOX.WIDTH / 2 + i * (L.BOX.WIDTH + gap);
    boxPositions.push({ cx: cx_i, cy });
  }

  // --- Draw arrows and operation labels between boxes ---
  for (let i = 0; i < n - 1; i++) {
    const x1 = boxPositions[i].cx + L.BOX.WIDTH / 2;
    const x2 = boxPositions[i + 1].cx - L.BOX.WIDTH / 2;
    const midX = (x1 + x2) / 2;

    // Arrow line
    ctx.beginPath();
    ctx.moveTo(x1 + 4, cy);
    ctx.lineTo(x2 - 4, cy);
    ctx.strokeStyle = L.ARROW.COLOR;
    ctx.lineWidth = L.ARROW.LINE_WIDTH;
    ctx.stroke();

    // Arrow head
    const headX = x2 - 4;
    ctx.beginPath();
    ctx.moveTo(headX - L.ARROW.HEAD_SIZE, cy - L.ARROW.HEAD_SIZE);
    ctx.lineTo(headX, cy);
    ctx.lineTo(headX - L.ARROW.HEAD_SIZE, cy + L.ARROW.HEAD_SIZE);
    ctx.strokeStyle = L.ARROW.COLOR;
    ctx.lineWidth = L.ARROW.LINE_WIDTH;
    ctx.stroke();

    // Operation pill label
    const pillText = OPERATIONS[i];
    ctx.font = L.PILL.FONT;
    const tw = ctx.measureText(pillText).width;
    const pillW = tw + 16;
    const pillY = cy - L.BOX.HEIGHT / 2 - 20;

    ctx.beginPath();
    const pillX = midX - pillW / 2;
    const pillH = L.PILL.HEIGHT;
    const pillR = L.PILL.RADIUS;
    ctx.moveTo(pillX + pillR, pillY);
    ctx.lineTo(pillX + pillW - pillR, pillY);
    ctx.quadraticCurveTo(pillX + pillW, pillY, pillX + pillW, pillY + pillR);
    ctx.lineTo(pillX + pillW, pillY + pillH - pillR);
    ctx.quadraticCurveTo(pillX + pillW, pillY + pillH, pillX + pillW - pillR, pillY + pillH);
    ctx.lineTo(pillX + pillR, pillY + pillH);
    ctx.quadraticCurveTo(pillX, pillY + pillH, pillX, pillY + pillH - pillR);
    ctx.lineTo(pillX, pillY + pillR);
    ctx.quadraticCurveTo(pillX, pillY, pillX + pillR, pillY);
    ctx.closePath();
    ctx.fillStyle = L.COLORS.pillBg;
    ctx.fill();

    ctx.font = L.FONTS.pill;
    ctx.fillStyle = L.COLORS.pillText;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText(pillText, midX, pillY + pillH / 2);
  }

  // --- Draw element boxes ---
  for (let i = 0; i < n; i++) {
    const el = ELEMENTS[i];
    const { cx: bx, cy: by } = boxPositions[i];
    const isSelected = i === selected;
    const isHovered = i === hovered && !isSelected;

    const x = bx - L.BOX.WIDTH / 2;
    const y = by - L.BOX.HEIGHT / 2;
    const bw = L.BOX.WIDTH;
    const bh = L.BOX.HEIGHT;
    const br = L.BOX.RADIUS;

    // Glow for selected element
    if (isSelected) {
      const g = ctx.createRadialGradient(bx, by, bh / 2, bx, by, bh * L.GLOW.RADIUS_MULTIPLIER);
      const glowHex = L.GLOW.ALPHA.toString(16).padStart(2, '0');
      g.addColorStop(0, el.color + glowHex);
      g.addColorStop(1, el.color + '00');
      ctx.beginPath();
      ctx.arc(bx, by, bh * L.GLOW.RADIUS_MULTIPLIER, 0, Math.PI * 2);
      ctx.fillStyle = g;
      ctx.fill();
    }

    // Rounded rect
    ctx.beginPath();
    ctx.moveTo(x + br, y);
    ctx.lineTo(x + bw - br, y);
    ctx.quadraticCurveTo(x + bw, y, x + bw, y + br);
    ctx.lineTo(x + bw, y + bh - br);
    ctx.quadraticCurveTo(x + bw, y + bh, x + bw - br, y + bh);
    ctx.lineTo(x + br, y + bh);
    ctx.quadraticCurveTo(x, y + bh, x, y + bh - br);
    ctx.lineTo(x, y + br);
    ctx.quadraticCurveTo(x, y, x + br, y);
    ctx.closePath();

    const fillAlpha = isSelected ? '25' : '10';
    ctx.fillStyle = el.color + fillAlpha;
    ctx.fill();

    const strokeAlpha = isSelected ? '' : isHovered ? 'aa' : '66';
    ctx.strokeStyle = el.color + strokeAlpha;
    ctx.lineWidth = isSelected ? L.COLORS.selectedStroke : L.COLORS.defaultStroke;
    ctx.stroke();

    // Label text
    ctx.font = L.FONTS.label;
    ctx.fillStyle = el.color + (isSelected ? '' : 'cc');
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText(el.label, bx, by - 6);

    // Sub text
    ctx.font = L.FONTS.sub;
    ctx.fillStyle = L.COLORS.subText;
    ctx.fillText(el.sub, bx, by + 10);
  }

  // Reset text baseline
  ctx.textBaseline = 'alphabetic';
}

// ---------------------------------------------------------------------------
// Detail panel
// ---------------------------------------------------------------------------

function renderDetail(detailEl: HTMLElement, index: number): void {
  const el = ELEMENTS[index];
  detailEl.innerHTML =
    `<strong style="color:${el.color}">${el.label}</strong> ` +
    `<span style="color:#6b7280">(${el.sub})</span><br>` +
    `${el.detail}`;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Initialize the identity diagram canvas.
 *
 * @param canvas   The <canvas> element to draw on.
 * @param detailEl The element to render element detail text into.
 *
 * Gracefully skips rendering if the canvas 2D context cannot be obtained.
 * Wraps draw calls in try/catch to prevent uncaught exceptions (Req 3.3, 22.6).
 */
export function initIdentityDiagram(
  canvas: HTMLCanvasElement,
  detailEl: HTMLElement,
): void {
  const ctx = canvas.getContext('2d');
  if (!ctx) {
    console.warn('[identity-diagram] canvas context unavailable');
    return;
  }

  let selected = 0;
  let hovered = -1;

  function safeDraw(): void {
    try {
      drawIdentityDiagram(ctx!, canvas.width, canvas.height, selected, hovered);
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

  // --- Canvas click handler ---
  canvas.addEventListener('click', (e: MouseEvent) => {
    const rect = canvas.getBoundingClientRect();
    const x = (e.clientX - rect.left) * (canvas.width / rect.width);
    const n = ELEMENTS.length;
    const usableW = canvas.width - IDENTITY_LAYOUT.PADDING.left - IDENTITY_LAYOUT.PADDING.right;
    const gap = (usableW - n * IDENTITY_LAYOUT.BOX.WIDTH) / (n - 1);

    for (let i = 0; i < n; i++) {
      const cx = IDENTITY_LAYOUT.PADDING.left + IDENTITY_LAYOUT.BOX.WIDTH / 2 + i * (IDENTITY_LAYOUT.BOX.WIDTH + gap);
      const left = cx - IDENTITY_LAYOUT.BOX.WIDTH / 2;
      const right = cx + IDENTITY_LAYOUT.BOX.WIDTH / 2;
      if (x >= left && x <= right) {
        selectElement(i);
        return;
      }
    }
  });

  // --- Canvas hover handler ---
  canvas.addEventListener('mousemove', (e: MouseEvent) => {
    const rect = canvas.getBoundingClientRect();
    const x = (e.clientX - rect.left) * (canvas.width / rect.width);
    const n = ELEMENTS.length;
    const usableW = canvas.width - IDENTITY_LAYOUT.PADDING.left - IDENTITY_LAYOUT.PADDING.right;
    const gap = (usableW - n * IDENTITY_LAYOUT.BOX.WIDTH) / (n - 1);

    let newHovered = -1;
    for (let i = 0; i < n; i++) {
      const cx = IDENTITY_LAYOUT.PADDING.left + IDENTITY_LAYOUT.BOX.WIDTH / 2 + i * (IDENTITY_LAYOUT.BOX.WIDTH + gap);
      const left = cx - IDENTITY_LAYOUT.BOX.WIDTH / 2;
      const right = cx + IDENTITY_LAYOUT.BOX.WIDTH / 2;
      if (x >= left && x <= right) {
        newHovered = i;
        break;
      }
    }

    if (newHovered !== hovered) {
      hovered = newHovered;
      safeDraw();
    }
  });

  canvas.addEventListener('mouseleave', () => {
    hovered = -1;
    safeDraw();
  });

  // --- Initial render ---
  safeDraw();
  renderDetail(detailEl, 0);
}
