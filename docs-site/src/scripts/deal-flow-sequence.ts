/**
 * Animated sequence diagram for the deal flow page.
 * Shows the message exchange between Requester and Provider:
 *   1. Requester → Provider: "request quote"
 *   2. Provider → Requester: "signed quote"
 *   3. Requester → Provider: "signed deal"
 *   4. Provider → Requester: "execute + receipt"
 *
 * Includes pause/play control (Req 22.4).
 * Null-check guard for canvas context and try/catch around draw (Req 3.3, 22.6).
 *
 * Validates: Requirements 22.2, 22.4
 */

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

export const SEQUENCE_LAYOUT = {
  PADDING: { top: 36, right: 30, bottom: 20, left: 30 },
  /** Lifeline header box */
  HEADER: {
    WIDTH: 100,
    HEIGHT: 30,
    RADIUS: 6,
    FONT: 'bold 12px system-ui, sans-serif',
  },
  /** Lifeline vertical line */
  LIFELINE: {
    DASH: [4, 4],
    COLOR: '#30363d',
    WIDTH: 1.2,
  },
  /** Arrow settings */
  ARROW: {
    HEAD_SIZE: 6,
    LINE_WIDTH: 1.8,
    LABEL_FONT: '11px system-ui, sans-serif',
    LABEL_OFFSET_Y: -8,
  },
  /** Animation timing */
  ANIM: {
    /** Duration per arrow in ms */
    ARROW_DURATION: 900,
    /** Pause between arrows in ms */
    ARROW_GAP: 400,
    /** Pause after full cycle before restart in ms */
    CYCLE_PAUSE: 1600,
  },
  COLORS: {
    bg: 'transparent',
    requester: '#a78bfa',
    provider: '#4ade80',
    arrowColors: ['#818cf8', '#67e8f9', '#f59e0b', '#4ade80'],
    labelText: '#d1d5db',
    headerText: '#e5e7eb',
  },
} as const;

// ---------------------------------------------------------------------------
// Message data
// ---------------------------------------------------------------------------

export interface SequenceMessage {
  from: 'requester' | 'provider';
  to: 'requester' | 'provider';
  label: string;
  color: string;
}

export const MESSAGES: SequenceMessage[] = [
  { from: 'requester', to: 'provider', label: 'request quote', color: SEQUENCE_LAYOUT.COLORS.arrowColors[0] },
  { from: 'provider', to: 'requester', label: 'signed quote', color: SEQUENCE_LAYOUT.COLORS.arrowColors[1] },
  { from: 'requester', to: 'provider', label: 'signed deal', color: SEQUENCE_LAYOUT.COLORS.arrowColors[2] },
  { from: 'provider', to: 'requester', label: 'execute + receipt', color: SEQUENCE_LAYOUT.COLORS.arrowColors[3] },
];

// ---------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------

function drawRoundedRect(
  ctx: CanvasRenderingContext2D,
  x: number, y: number, w: number, h: number, r: number,
): void {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.lineTo(x + w - r, y);
  ctx.quadraticCurveTo(x + w, y, x + w, y + r);
  ctx.lineTo(x + w, y + h - r);
  ctx.quadraticCurveTo(x + w, y + h, x + w - r, y + h);
  ctx.lineTo(x + r, y + h);
  ctx.quadraticCurveTo(x, y + h, x, y + h - r);
  ctx.lineTo(x, y + r);
  ctx.quadraticCurveTo(x, y, x + r, y);
  ctx.closePath();
}

function lifelineX(role: 'requester' | 'provider', w: number): number {
  const L = SEQUENCE_LAYOUT;
  return role === 'requester'
    ? L.PADDING.left + L.HEADER.WIDTH / 2
    : w - L.PADDING.right - L.HEADER.WIDTH / 2;
}

// ---------------------------------------------------------------------------
// Main draw
// ---------------------------------------------------------------------------

function drawSequence(
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  progress: number, // 0..1 across the full cycle (all arrows)
): void {
  const L = SEQUENCE_LAYOUT;
  const n = MESSAGES.length;

  ctx.clearRect(0, 0, w, h);

  const reqX = lifelineX('requester', w);
  const provX = lifelineX('provider', w);

  // --- Draw lifeline headers ---
  for (const [label, cx, color] of [
    ['Requester', reqX, L.COLORS.requester],
    ['Provider', provX, L.COLORS.provider],
  ] as const) {
    const hx = cx - L.HEADER.WIDTH / 2;
    const hy = L.PADDING.top - L.HEADER.HEIGHT - 4;
    drawRoundedRect(ctx, hx, hy, L.HEADER.WIDTH, L.HEADER.HEIGHT, L.HEADER.RADIUS);
    ctx.fillStyle = color + '18';
    ctx.fill();
    ctx.strokeStyle = color + '66';
    ctx.lineWidth = 1;
    ctx.stroke();

    ctx.font = L.HEADER.FONT;
    ctx.fillStyle = L.COLORS.headerText;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText(label, cx, hy + L.HEADER.HEIGHT / 2);
  }

  // --- Draw lifeline dashed lines ---
  const lifeTop = L.PADDING.top;
  const lifeBot = h - L.PADDING.bottom;

  ctx.setLineDash(L.LIFELINE.DASH);
  ctx.strokeStyle = L.LIFELINE.COLOR;
  ctx.lineWidth = L.LIFELINE.WIDTH;
  for (const x of [reqX, provX]) {
    ctx.beginPath();
    ctx.moveTo(x, lifeTop);
    ctx.lineTo(x, lifeBot);
    ctx.stroke();
  }
  ctx.setLineDash([]);

  // --- Draw message arrows based on progress ---
  const arrowSpacing = (lifeBot - lifeTop) / (n + 1);

  for (let i = 0; i < n; i++) {
    const msg = MESSAGES[i];
    // Each arrow occupies 1/n of the progress range
    const arrowStart = i / n;
    const arrowEnd = (i + 1) / n;
    const arrowProgress = Math.max(0, Math.min(1, (progress - arrowStart) / (arrowEnd - arrowStart)));

    if (arrowProgress <= 0) continue;

    const y = lifeTop + arrowSpacing * (i + 1);
    const fromX = lifelineX(msg.from, w);
    const toX = lifelineX(msg.to, w);
    const dir = toX > fromX ? 1 : -1;

    // Interpolate arrow tip position
    const currentTipX = fromX + (toX - fromX) * arrowProgress;

    // Draw arrow line
    ctx.beginPath();
    ctx.moveTo(fromX, y);
    ctx.lineTo(currentTipX, y);
    ctx.strokeStyle = msg.color;
    ctx.lineWidth = L.ARROW.LINE_WIDTH;
    ctx.stroke();

    // Draw arrow head (only when arrow has progressed enough)
    if (arrowProgress > 0.1) {
      ctx.beginPath();
      ctx.moveTo(currentTipX - dir * L.ARROW.HEAD_SIZE, y - L.ARROW.HEAD_SIZE);
      ctx.lineTo(currentTipX, y);
      ctx.lineTo(currentTipX - dir * L.ARROW.HEAD_SIZE, y + L.ARROW.HEAD_SIZE);
      ctx.strokeStyle = msg.color;
      ctx.lineWidth = L.ARROW.LINE_WIDTH;
      ctx.stroke();
    }

    // Draw label (fade in as arrow progresses)
    if (arrowProgress > 0.3) {
      const labelAlpha = Math.min(1, (arrowProgress - 0.3) / 0.3);
      const midX = (fromX + toX) / 2;
      ctx.font = L.ARROW.LABEL_FONT;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'alphabetic';
      ctx.fillStyle = L.COLORS.labelText + Math.round(labelAlpha * 255).toString(16).padStart(2, '0');
      ctx.fillText(msg.label, midX, y + L.ARROW.LABEL_OFFSET_Y);
    }
  }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Initialize the deal flow sequence diagram with animation and pause/play.
 *
 * @param canvas       The <canvas> element to draw on.
 * @param playPauseBtn The button element that toggles pause/play.
 *
 * Gracefully skips rendering if the canvas 2D context cannot be obtained.
 * Wraps draw calls in try/catch (Req 3.3, 22.6).
 */
export function initDealFlowSequence(
  canvas: HTMLCanvasElement,
  playPauseBtn: HTMLButtonElement,
): void {
  const ctx = canvas.getContext('2d');
  if (!ctx) {
    console.warn('[deal-flow-sequence] canvas context unavailable');
    return;
  }

  const n = MESSAGES.length;
  const L = SEQUENCE_LAYOUT;
  const totalArrowTime = n * L.ANIM.ARROW_DURATION + (n - 1) * L.ANIM.ARROW_GAP;
  const cycleDuration = totalArrowTime + L.ANIM.CYCLE_PAUSE;

  let playing = true;
  let startTime = performance.now();
  let pausedAt = 0;
  let animFrameId: number | null = null;

  function timeToProgress(elapsed: number): number {
    // Map elapsed time within a cycle to 0..1 progress across arrows
    const t = elapsed % cycleDuration;
    if (t >= totalArrowTime) return 1; // in the pause gap

    // Walk through arrows to find which one we're in
    let accum = 0;
    for (let i = 0; i < n; i++) {
      const arrowEnd = accum + L.ANIM.ARROW_DURATION;
      if (t < arrowEnd) {
        // Within arrow i
        const arrowLocal = (t - accum) / L.ANIM.ARROW_DURATION;
        return (i + arrowLocal) / n;
      }
      accum = arrowEnd;
      if (i < n - 1) {
        const gapEnd = accum + L.ANIM.ARROW_GAP;
        if (t < gapEnd) {
          // In the gap after arrow i — show arrow i complete
          return (i + 1) / n;
        }
        accum = gapEnd;
      }
    }
    return 1;
  }

  function tick(now: number): void {
    try {
      const elapsed = now - startTime;
      const progress = timeToProgress(elapsed);
      drawSequence(ctx!, canvas.width, canvas.height, progress);
    } catch (err) {
      console.error('[deal-flow-sequence] draw error:', err);
    }
    if (playing) {
      animFrameId = requestAnimationFrame(tick);
    }
  }

  // --- Pause / Play ---
  function updateButtonLabel(): void {
    playPauseBtn.textContent = playing ? '⏸ Pause' : '▶ Play';
    playPauseBtn.setAttribute('aria-label', playing ? 'Pause animation' : 'Play animation');
  }

  playPauseBtn.addEventListener('click', () => {
    if (playing) {
      // Pause
      playing = false;
      pausedAt = performance.now();
      if (animFrameId !== null) {
        cancelAnimationFrame(animFrameId);
        animFrameId = null;
      }
    } else {
      // Resume — shift startTime so elapsed stays continuous
      playing = true;
      startTime += performance.now() - pausedAt;
      animFrameId = requestAnimationFrame(tick);
    }
    updateButtonLabel();
  });

  // --- Start ---
  updateButtonLabel();
  animFrameId = requestAnimationFrame(tick);
}
