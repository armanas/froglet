/**
 * Deal flow chain canvas renderer.
 * Renders the six artifact types as a connected evidence chain on the deal-flow page.
 */

export interface ChainArtifact {
  label: string;
  sub: string;
  color: string;
  signer: 'provider' | 'requester';
  purpose: string;
  hashLink: string;
}

export const ARTIFACTS: ChainArtifact[] = [
  {
    label: 'Descriptor',
    sub: 'who',
    color: '#52c72a',
    signer: 'provider',
    purpose: 'Declares identity, capabilities, and transport endpoints.',
    hashLink: '(chain root — no parent hash)',
  },
  {
    label: 'Offer',
    sub: 'what',
    color: '#4ea3ff',
    signer: 'provider',
    purpose: 'Specific service with pricing and execution profile.',
    hashLink: 'References descriptor by SHA-256 hash.',
  },
  {
    label: 'Quote',
    sub: 'price',
    color: '#67e8f9',
    signer: 'provider',
    purpose: 'Prices a workload for a specific requester. Ephemeral — has an expiry.',
    hashLink: 'References offer by SHA-256 hash.',
  },
  {
    label: 'Deal',
    sub: 'commit',
    color: '#9aa497',
    signer: 'requester',
    purpose: 'Requester commits to the quote. The only artifact signed by the requester.',
    hashLink: 'References quote by SHA-256 hash.',
  },
  {
    label: 'Invoice',
    sub: 'payment',
    color: '#f5c518',
    signer: 'provider',
    purpose: 'Two Lightning invoices — base fee (locks upfront) and success fee (settles on completion).',
    hashLink: 'References deal by SHA-256 hash.',
  },
  {
    label: 'Receipt',
    sub: 'proof',
    color: '#7ad954',
    signer: 'provider',
    purpose: 'Terminal artifact. Cryptographic proof of execution, result hash, and settlement state.',
    hashLink: 'References invoice bundle by SHA-256 hash.',
  },
];

const LAYOUT = {
  padding: { top: 28, right: 20, bottom: 26, left: 20 },
  node: { width: 96, height: 58, radius: 12 },
  arrowHead: 7,
  animationMs: 260,
  fonts: {
    label: '700 12px "JetBrains Mono", ui-monospace, monospace',
    sub: '500 10px "Inter", system-ui, sans-serif',
    signer: '700 9px "JetBrains Mono", ui-monospace, monospace',
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

interface AnimationState {
  from: number;
  to: number;
  startTime: number;
  progress: number;
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
  const w = Math.max(340, Math.round(rect.width || Number(canvas.getAttribute('width')) || 760));
  const h = Math.max(150, Math.round(rect.height || Number(canvas.getAttribute('height')) || 170));
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

function easeOut(t: number): number {
  return 1 - Math.pow(1 - t, 3);
}

function nodeLayout(w: number, h: number): Array<{ x: number; y: number; w: number; h: number; cx: number; cy: number }> {
  const n = ARTIFACTS.length;
  const slot = (w - LAYOUT.padding.left - LAYOUT.padding.right) / n;
  const nodeW = Math.min(LAYOUT.node.width, Math.max(62, slot - 12));
  const nodeH = LAYOUT.node.height;
  const y = h / 2 - nodeH / 2 + 8;
  return ARTIFACTS.map((_, i) => {
    const cx = LAYOUT.padding.left + slot * i + slot / 2;
    return { x: cx - nodeW / 2, y, w: nodeW, h: nodeH, cx, cy: y + nodeH / 2 };
  });
}

function selectedAlpha(selected: number, anim: AnimationState | null, index: number): number {
  if (!anim) return selected === index ? 1 : 0;
  const eased = easeOut(anim.progress);
  const from = anim.from === index ? 1 - eased : 0;
  const to = anim.to === index ? eased : 0;
  return Math.max(from, to);
}

function drawChain(
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  selected: number,
  hovered: number,
  anim: AnimationState | null,
  palette: Palette,
): void {
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = palette.surface;
  ctx.fillRect(0, 0, w, h);

  const boxes = nodeLayout(w, h);

  for (let i = 0; i < boxes.length - 1; i++) {
    const from = boxes[i];
    const to = boxes[i + 1];
    const x1 = from.x + from.w + 8;
    const x2 = to.x - 8;
    const y = from.cy;
    ctx.strokeStyle = palette.borderStrong;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(x1, y);
    ctx.lineTo(x2, y);
    ctx.stroke();
    ctx.beginPath();
    ctx.moveTo(x2 - LAYOUT.arrowHead, y - LAYOUT.arrowHead);
    ctx.lineTo(x2, y);
    ctx.lineTo(x2 - LAYOUT.arrowHead, y + LAYOUT.arrowHead);
    ctx.stroke();
  }

  for (let i = 0; i < ARTIFACTS.length; i++) {
    const it = ARTIFACTS[i];
    const box = boxes[i];
    const alpha = selectedAlpha(selected, anim, i);
    const active = alpha > 0.45;
    const hover = hovered === i && !active;

    if (alpha > 0.01) {
      const glow = ctx.createRadialGradient(box.cx, box.cy, box.h / 3, box.cx, box.cy, box.w * 0.95);
      glow.addColorStop(0, `${it.color}${Math.round(alpha * 0x35).toString(16).padStart(2, '0')}`);
      glow.addColorStop(1, `${it.color}00`);
      ctx.fillStyle = glow;
      ctx.beginPath();
      ctx.arc(box.cx, box.cy, box.w * 0.95, 0, Math.PI * 2);
      ctx.fill();
    }

    roundedRect(ctx, box.x, box.y, box.w, box.h, LAYOUT.node.radius);
    ctx.fillStyle = active ? `${it.color}1f` : palette.surface2;
    ctx.fill();
    ctx.strokeStyle = active || hover ? it.color : palette.borderStrong;
    ctx.lineWidth = active ? 2.5 : 1.5;
    ctx.stroke();

    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.font = LAYOUT.fonts.signer;
    ctx.fillStyle = active || hover ? it.color : palette.fg3;
    ctx.fillText(it.signer, box.cx, box.y + 12);
    ctx.font = LAYOUT.fonts.label;
    ctx.fillStyle = active || hover ? it.color : palette.fg1;
    ctx.fillText(it.label, box.cx, box.cy + 1);
    ctx.font = LAYOUT.fonts.sub;
    ctx.fillStyle = palette.fg2;
    ctx.fillText(it.sub, box.cx, box.y + box.h - 11);
  }
}

function renderDetail(detailEl: HTMLElement, index: number): void {
  const it = ARTIFACTS[index];
  const hashNote = index > 0
    ? ` <span class="plot-muted">← ${it.hashLink}</span>`
    : ` <span class="plot-muted">${it.hashLink}</span>`;
  detailEl.innerHTML =
    `<strong style="color:${it.color}">${it.label}</strong> ` +
    `<span style="color:${it.color}">(signed by ${it.signer})</span><br>` +
    `${it.purpose}${hashNote}`;
}

export function initChainCanvas(canvas: HTMLCanvasElement, detailEl: HTMLElement, buttonRow: HTMLElement): void {
  const ctx = canvas.getContext('2d');
  if (!ctx) {
    console.warn('[chain-canvas] canvas context unavailable');
    return;
  }

  const n = ARTIFACTS.length;
  let selected = 0;
  let hovered = -1;
  let animation: AnimationState | null = null;
  let rafId: number | null = null;
  let viewport = { w: Number(canvas.getAttribute('width')) || 760, h: Number(canvas.getAttribute('height')) || 170 };

  buttonRow.innerHTML = ARTIFACTS.map(
    (it, i) => `<button type="button" class="plot-button${i === 0 ? ' is-active' : ''}" data-index="${i}">${it.label}</button>`,
  ).join('');

  function safeDraw(): void {
    try {
      viewport = resizeCanvas(canvas, ctx!);
      drawChain(ctx!, viewport.w, viewport.h, selected, hovered, animation, readPalette());
    } catch (err) {
      console.error('[chain-canvas] draw error:', err);
    }
  }

  function animationTick(now: number): void {
    if (!animation) return;
    animation.progress = Math.min(1, (now - animation.startTime) / LAYOUT.animationMs);
    safeDraw();
    if (animation.progress < 1) {
      rafId = requestAnimationFrame(animationTick);
    } else {
      animation = null;
      rafId = null;
      safeDraw();
    }
  }

  function selectArtifact(index: number): void {
    if (index < 0 || index >= n || index === selected) return;
    const prev = selected;
    selected = index;
    animation = { from: prev, to: index, startTime: performance.now(), progress: 0 };
    if (rafId !== null) cancelAnimationFrame(rafId);
    rafId = requestAnimationFrame(animationTick);
    renderDetail(detailEl, index);
    buttonRow.querySelectorAll('button').forEach((btn, idx) => {
      btn.classList.toggle('is-active', idx === index);
    });
  }

  function indexFromPointer(event: MouseEvent): number {
    const rect = canvas.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const slot = viewport.w / n;
    return Math.max(0, Math.min(n - 1, Math.floor(x / slot)));
  }

  canvas.addEventListener('click', (event) => selectArtifact(indexFromPointer(event)));
  canvas.addEventListener('mousemove', (event) => {
    const next = indexFromPointer(event);
    if (next !== hovered) {
      hovered = next;
      if (!animation) safeDraw();
    }
  });
  canvas.addEventListener('mouseleave', () => {
    hovered = -1;
    if (!animation) safeDraw();
  });
  buttonRow.addEventListener('click', (event: Event) => {
    const button = (event.target as HTMLElement).closest('button[data-index]') as HTMLElement | null;
    if (!button) return;
    const idx = Number(button.dataset.index);
    if (!Number.isNaN(idx)) selectArtifact(idx);
  });
  window.addEventListener('resize', safeDraw, { passive: true });

  safeDraw();
  renderDetail(detailEl, 0);
}
