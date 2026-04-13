/**
 * Deal flow chain canvas renderer.
 * Renders the six artifact types as a connected chain on the deal-flow learn page.
 *
 * Artifact chain (Kernel_Spec):
 *   descriptor → offer → quote → deal → invoice_bundle → receipt
 *
 * Each artifact references the previous by SHA-256 hash.
 * Five are signed by the provider; the deal is signed by the requester.
 *
 * Validates: Requirements 20.1, 20.2, 20.3, 20.4
 */

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

export interface ChainArtifact {
  label: string;
  sub: string;
  color: string;
  signer: 'provider' | 'requester';
  purpose: string;
  hashLink: string;
}

/** The six artifact types in protocol order. */
export const ARTIFACTS: ChainArtifact[] = [
  {
    label: 'Descriptor',
    sub: 'who',
    color: '#a78bfa',
    signer: 'provider',
    purpose: 'Declares identity, capabilities, and transport endpoints.',
    hashLink: '(chain root — no parent hash)',
  },
  {
    label: 'Offer',
    sub: 'what',
    color: '#818cf8',
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
    color: '#93c5fd',
    signer: 'requester',
    purpose: 'Requester commits to the quote. The only artifact signed by the requester.',
    hashLink: 'References quote by SHA-256 hash.',
  },
  {
    label: 'Invoice',
    sub: 'payment',
    color: '#fbbf24',
    signer: 'provider',
    purpose: 'Two Lightning invoices — base fee (locks upfront) and success fee (settles on completion).',
    hashLink: 'References deal by SHA-256 hash.',
  },
  {
    label: 'Receipt',
    sub: 'proof',
    color: '#4ade80',
    signer: 'provider',
    purpose: 'Terminal artifact. Cryptographic proof of execution, result hash, and settlement state.',
    hashLink: 'References invoice bundle by SHA-256 hash.',
  },
];

// ---------------------------------------------------------------------------
// Layout constants (no magic numbers)
// ---------------------------------------------------------------------------

export const CHAIN_LAYOUT = {
  /** Radius of the artifact circle when selected */
  RADIUS_SELECTED: 26,
  /** Radius of the artifact circle when hovered */
  RADIUS_HOVERED: 24,
  /** Radius of the artifact circle in default state */
  RADIUS_DEFAULT: 22,
  /** Glow radius multiplier for selected artifact */
  GLOW_MULTIPLIER: 2,
  /** Arrow connector gap from box edge */
  ARROW_GAP: 8,
  /** Arrow head half-size */
  ARROW_HEAD: 4,
  /** Arrow connector line width */
  ARROW_LINE_WIDTH: 2,
  /** Arrow head line width */
  ARROW_HEAD_LINE_WIDTH: 1.5,
  /** Vertical offset for sub-label below circle */
  SUB_LABEL_OFFSET: 14,
  /** Vertical offset for signer label above circle */
  SIGNER_LABEL_OFFSET: 6,
  /** Stroke width for selected circle */
  STROKE_SELECTED: 2,
  /** Stroke width for default circle */
  STROKE_DEFAULT: 1.2,
  /** Animation duration in ms for transitions */
  ANIMATION_DURATION_MS: 300,
  FONTS: {
    label: 'bold 9px system-ui',
    sub: '8px system-ui',
    signer: '7px system-ui',
  },
  COLORS: {
    arrow: '#30363d',
    subLabel: '#6b7280',
    /** Provider signer indicator color */
    providerSigner: '#a78bfa',
    /** Requester signer indicator color */
    requesterSigner: '#93c5fd',
    /** Provider signer indicator alpha suffix */
    providerSignerAlpha: 'cc',
    /** Requester signer indicator alpha suffix */
    requesterSignerAlpha: 'cc',
  },
} as const;

// ---------------------------------------------------------------------------
// Animation state
// ---------------------------------------------------------------------------

interface AnimationState {
  from: number;
  to: number;
  startTime: number;
  progress: number; // 0..1
}

// ---------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------

function easeInOutCubic(t: number): number {
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

function lerpRadius(fromIdx: number, toIdx: number, progress: number, idx: number): number {
  const L = CHAIN_LAYOUT;
  const fromR = fromIdx === idx ? L.RADIUS_SELECTED : L.RADIUS_DEFAULT;
  const toR = toIdx === idx ? L.RADIUS_SELECTED : L.RADIUS_DEFAULT;
  return fromR + (toR - fromR) * progress;
}

function lerpAlpha(fromIdx: number, toIdx: number, progress: number, idx: number): number {
  const fromA = fromIdx === idx ? 1.0 : 0.0;
  const toA = toIdx === idx ? 1.0 : 0.0;
  return fromA + (toA - fromA) * progress;
}

/**
 * Draw the full chain on the canvas.
 */
function drawChain(
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  selected: number,
  hovered: number,
  anim: AnimationState | null,
): void {
  const L = CHAIN_LAYOUT;
  const n = ARTIFACTS.length;
  const bw = w / n;

  ctx.clearRect(0, 0, w, h);

  // Compute eased progress
  const easedProgress = anim ? easeInOutCubic(anim.progress) : 1;

  // --- Arrows between artifacts ---
  for (let i = 0; i < n - 1; i++) {
    const x1 = (i + 1) * bw - L.ARROW_GAP;
    const x2 = (i + 1) * bw + L.ARROW_GAP;

    // Connector line
    ctx.beginPath();
    ctx.moveTo(x1, h / 2);
    ctx.lineTo(x2, h / 2);
    ctx.strokeStyle = L.COLORS.arrow;
    ctx.lineWidth = L.ARROW_LINE_WIDTH;
    ctx.stroke();

    // Arrow head
    ctx.beginPath();
    ctx.moveTo(x2 - L.ARROW_HEAD, h / 2 - L.ARROW_HEAD);
    ctx.lineTo(x2, h / 2);
    ctx.lineTo(x2 - L.ARROW_HEAD, h / 2 + L.ARROW_HEAD);
    ctx.strokeStyle = L.COLORS.arrow;
    ctx.lineWidth = L.ARROW_HEAD_LINE_WIDTH;
    ctx.stroke();
  }

  // --- Artifact circles ---
  for (let i = 0; i < n; i++) {
    const it = ARTIFACTS[i];
    const cx = i * bw + bw / 2;
    const cy = h / 2;

    // Determine radius and selection alpha
    let r: number;
    let selAlpha: number;

    if (anim && anim.progress < 1) {
      r = lerpRadius(anim.from, anim.to, easedProgress, i);
      selAlpha = lerpAlpha(anim.from, anim.to, easedProgress, i);
    } else {
      const isS = i === selected;
      const isH = i === hovered;
      r = isS ? L.RADIUS_SELECTED : isH ? L.RADIUS_HOVERED : L.RADIUS_DEFAULT;
      selAlpha = isS ? 1.0 : 0.0;
    }

    // Glow for selected/transitioning artifact
    if (selAlpha > 0.01) {
      const g = ctx.createRadialGradient(cx, cy, r, cx, cy, r * L.GLOW_MULTIPLIER);
      const glowAlpha = Math.round(selAlpha * 0x20).toString(16).padStart(2, '0');
      g.addColorStop(0, it.color + glowAlpha);
      g.addColorStop(1, it.color + '00');
      ctx.beginPath();
      ctx.arc(cx, cy, r * L.GLOW_MULTIPLIER, 0, Math.PI * 2);
      ctx.fillStyle = g;
      ctx.fill();
    }

    // Circle fill + stroke
    const fillAlpha = selAlpha > 0.5 ? '25' : '10';
    const strokeAlpha = selAlpha > 0.5 ? '' : '66';
    const strokeW = selAlpha > 0.5 ? L.STROKE_SELECTED : L.STROKE_DEFAULT;

    ctx.beginPath();
    ctx.arc(cx, cy, r, 0, Math.PI * 2);
    ctx.fillStyle = it.color + fillAlpha;
    ctx.fill();
    ctx.strokeStyle = it.color + strokeAlpha;
    ctx.lineWidth = strokeW;
    ctx.stroke();

    // Label inside circle
    const labelAlpha = selAlpha > 0.5 ? '' : 'cc';
    ctx.font = L.FONTS.label;
    ctx.fillStyle = it.color + labelAlpha;
    ctx.textAlign = 'center';
    ctx.fillText(it.label, cx, cy + 3);

    // Sub-label below circle
    ctx.font = L.FONTS.sub;
    ctx.fillStyle = L.COLORS.subLabel;
    ctx.fillText(it.sub, cx, cy + r + L.SUB_LABEL_OFFSET);

    // Signer indicator above circle — distinct styling for provider vs requester
    const isProvider = it.signer === 'provider';
    ctx.font = L.FONTS.signer;
    ctx.fillStyle = isProvider
      ? L.COLORS.providerSigner + L.COLORS.providerSignerAlpha
      : L.COLORS.requesterSigner + L.COLORS.requesterSignerAlpha;
    ctx.fillText(it.signer, cx, cy - r - L.SIGNER_LABEL_OFFSET);
  }
}

// ---------------------------------------------------------------------------
// Detail panel
// ---------------------------------------------------------------------------

function renderDetail(detailEl: HTMLElement, index: number): void {
  const it = ARTIFACTS[index];
  const signerColor = it.signer === 'provider'
    ? CHAIN_LAYOUT.COLORS.providerSigner
    : CHAIN_LAYOUT.COLORS.requesterSigner;

  const hashNote = index > 0
    ? ` <span style="color:#4b5563">← ${it.hashLink}</span>`
    : ` <span style="color:#4b5563">${it.hashLink}</span>`;

  detailEl.innerHTML =
    `<strong style="color:${it.color}">${it.label}</strong> ` +
    `<span style="color:${signerColor}">(signed by ${it.signer})</span><br>` +
    `${it.purpose}${hashNote}`;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Initialize the chain canvas visualization.
 *
 * @param canvas   The <canvas> element to draw on.
 * @param detailEl The element to render artifact detail text into.
 * @param buttonRow The container element for navigation buttons.
 *
 * Gracefully skips rendering if the canvas 2D context cannot be obtained.
 * Wraps draw calls in try/catch to prevent uncaught exceptions.
 */
export function initChainCanvas(
  canvas: HTMLCanvasElement,
  detailEl: HTMLElement,
  buttonRow: HTMLElement,
): void {
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

  // --- Build buttons ---
  buttonRow.innerHTML = ARTIFACTS.map(
    (it, i) =>
      `<button type="button" class="plot-button${i === 0 ? ' is-active' : ''}" data-index="${i}">${it.label}</button>`,
  ).join('');

  // --- Draw wrapper with error handling ---
  function safeDraw(): void {
    try {
      drawChain(ctx!, canvas.width, canvas.height, selected, hovered, animation);
    } catch (err) {
      console.error('[chain-canvas] draw error:', err);
    }
  }

  // --- Animation loop ---
  function animationTick(now: number): void {
    if (!animation) return;

    const elapsed = now - animation.startTime;
    animation.progress = Math.min(1, elapsed / CHAIN_LAYOUT.ANIMATION_DURATION_MS);

    safeDraw();

    if (animation.progress < 1) {
      rafId = requestAnimationFrame(animationTick);
    } else {
      animation = null;
      rafId = null;
    }
  }

  // --- Select an artifact with animated transition ---
  function selectArtifact(index: number): void {
    if (index < 0 || index >= n || index === selected) return;

    const prev = selected;
    selected = index;

    // Start animation
    animation = {
      from: prev,
      to: index,
      startTime: performance.now(),
      progress: 0,
    };

    if (rafId !== null) cancelAnimationFrame(rafId);
    rafId = requestAnimationFrame(animationTick);

    // Update detail and buttons immediately
    renderDetail(detailEl, index);
    buttonRow.querySelectorAll('button').forEach((btn, idx) => {
      btn.classList.toggle('is-active', idx === index);
    });
  }

  // --- Canvas click handler ---
  canvas.addEventListener('click', (e: MouseEvent) => {
    const rect = canvas.getBoundingClientRect();
    const x = (e.clientX - rect.left) * (canvas.width / rect.width);
    const bw = canvas.width / n;
    const i = Math.floor(x / bw);
    if (i >= 0 && i < n) selectArtifact(i);
  });

  // --- Canvas hover handler ---
  canvas.addEventListener('mousemove', (e: MouseEvent) => {
    const rect = canvas.getBoundingClientRect();
    const x = (e.clientX - rect.left) * (canvas.width / rect.width);
    const bw = canvas.width / n;
    const i = Math.floor(x / bw);
    if (i !== hovered) {
      hovered = i;
      if (!animation) safeDraw();
    }
  });

  canvas.addEventListener('mouseleave', () => {
    hovered = -1;
    if (!animation) safeDraw();
  });

  // --- Button click handler ---
  buttonRow.addEventListener('click', (event: Event) => {
    const button = (event.target as HTMLElement).closest('button[data-index]') as HTMLElement | null;
    if (!button) return;
    const idx = Number(button.dataset.index);
    if (!isNaN(idx)) selectArtifact(idx);
  });

  // --- Initial render ---
  safeDraw();
  renderDetail(detailEl, 0);
}
