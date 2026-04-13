/**
 * Interactive trust threshold diagram.
 * The user sets a risk threshold k and sees which example
 * stake-to-deal-value ratios pass the safety check `stake / deal_value > k`.
 *
 * Passing ratios are highlighted green, failing ones red.
 * Real-time value display updates as the slider changes (Req 22.5).
 * Null-check guard for canvas context and try/catch around draw (Req 3.3, 22.6).
 *
 * Validates: Requirements 22.3, 22.5
 */

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

export const THRESHOLD_LAYOUT = {
  PADDING: { top: 28, right: 24, bottom: 28, left: 24 },
  /** Deal bar dimensions */
  BAR: {
    HEIGHT: 36,
    RADIUS: 6,
    GAP: 12,
    LABEL_FONT: 'bold 11px system-ui, sans-serif',
    VALUE_FONT: '10px system-ui, sans-serif',
  },
  /** Threshold line */
  LINE: {
    WIDTH: 1.5,
    DASH: [6, 4],
    COLOR: '#fbbf24',
    LABEL_FONT: 'bold 11px system-ui, sans-serif',
  },
  COLORS: {
    pass: '#4ade80',
    fail: '#f87171',
    passFill: '#4ade8020',
    failFill: '#f8717120',
    passStroke: '#4ade8066',
    failStroke: '#f8717166',
    labelText: '#e5e7eb',
    valueText: '#9ca3af',
    bg: 'transparent',
  },
} as const;

// ---------------------------------------------------------------------------
// Example deals
// ---------------------------------------------------------------------------

export interface ExampleDeal {
  label: string;
  stake: number;
  dealValue: number;
}

export const EXAMPLE_DEALS: ExampleDeal[] = [
  { label: 'Small task',     stake: 500,   dealValue: 50 },
  { label: 'API call',       stake: 1000,  dealValue: 200 },
  { label: 'Data pipeline',  stake: 3000,  dealValue: 800 },
  { label: 'ML inference',   stake: 2000,  dealValue: 1500 },
  { label: 'Bulk compute',   stake: 5000,  dealValue: 4000 },
  { label: 'High-value job', stake: 1000,  dealValue: 2000 },
];

// ---------------------------------------------------------------------------
// Pure computation
// ---------------------------------------------------------------------------

/**
 * Compute the stake-to-deal-value ratio.
 */
export function computeRatio(stake: number, dealValue: number): number {
  if (dealValue === 0) return Infinity;
  return stake / dealValue;
}

/**
 * Check whether a deal passes the safety threshold.
 */
export function passesSafetyCheck(stake: number, dealValue: number, k: number): boolean {
  return computeRatio(stake, dealValue) > k;
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

function drawThresholdDiagram(
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  threshold: number,
): void {
  const L = THRESHOLD_LAYOUT;
  const deals = EXAMPLE_DEALS;
  const n = deals.length;

  ctx.clearRect(0, 0, w, h);

  // Compute max ratio for scaling bars
  const ratios = deals.map(d => computeRatio(d.stake, d.dealValue));
  const maxRatio = Math.max(...ratios, threshold * 1.3, 2);

  const usableW = w - L.PADDING.left - L.PADDING.right;
  const usableH = h - L.PADDING.top - L.PADDING.bottom;
  const barTotalH = n * L.BAR.HEIGHT + (n - 1) * L.BAR.GAP;
  const startY = L.PADDING.top + (usableH - barTotalH) / 2;

  // --- Draw threshold vertical line ---
  const threshX = L.PADDING.left + (threshold / maxRatio) * usableW;

  ctx.save();
  ctx.setLineDash(L.LINE.DASH);
  ctx.strokeStyle = L.LINE.COLOR;
  ctx.lineWidth = L.LINE.WIDTH;
  ctx.beginPath();
  ctx.moveTo(threshX, L.PADDING.top - 8);
  ctx.lineTo(threshX, h - L.PADDING.bottom + 8);
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.restore();

  // Threshold label at top
  ctx.font = L.LINE.LABEL_FONT;
  ctx.fillStyle = L.LINE.COLOR;
  ctx.textAlign = 'center';
  ctx.textBaseline = 'bottom';
  ctx.fillText(`k = ${threshold.toFixed(1)}`, threshX, L.PADDING.top - 12);

  // --- Draw deal bars ---
  for (let i = 0; i < n; i++) {
    const deal = deals[i];
    const ratio = ratios[i];
    const passes = passesSafetyCheck(deal.stake, deal.dealValue, threshold);
    const y = startY + i * (L.BAR.HEIGHT + L.BAR.GAP);

    // Bar width proportional to ratio
    const barW = Math.max(8, (ratio / maxRatio) * usableW);
    const bx = L.PADDING.left;
    const by = y;
    const bw = barW;
    const bh = L.BAR.HEIGHT;
    const br = L.BAR.RADIUS;

    // Rounded rect
    ctx.beginPath();
    ctx.moveTo(bx + br, by);
    ctx.lineTo(bx + bw - br, by);
    ctx.quadraticCurveTo(bx + bw, by, bx + bw, by + br);
    ctx.lineTo(bx + bw, by + bh - br);
    ctx.quadraticCurveTo(bx + bw, by + bh, bx + bw - br, by + bh);
    ctx.lineTo(bx + br, by + bh);
    ctx.quadraticCurveTo(bx, by + bh, bx, by + bh - br);
    ctx.lineTo(bx, by + br);
    ctx.quadraticCurveTo(bx, by, bx + br, by);
    ctx.closePath();

    ctx.fillStyle = passes ? L.COLORS.passFill : L.COLORS.failFill;
    ctx.fill();
    ctx.strokeStyle = passes ? L.COLORS.passStroke : L.COLORS.failStroke;
    ctx.lineWidth = 1.2;
    ctx.stroke();

    // Deal label (inside bar, left-aligned)
    ctx.font = L.BAR.LABEL_FONT;
    ctx.fillStyle = L.COLORS.labelText;
    ctx.textAlign = 'left';
    ctx.textBaseline = 'middle';
    ctx.fillText(deal.label, bx + 10, by + bh / 2 - 6);

    // Ratio value (inside bar, below label)
    ctx.font = L.BAR.VALUE_FONT;
    ctx.fillStyle = L.COLORS.valueText;
    ctx.fillText(
      `ratio: ${ratio.toFixed(1)}  (${deal.stake}/${deal.dealValue})`,
      bx + 10,
      by + bh / 2 + 8,
    );

    // Pass/fail indicator at right end of bar
    const indicatorX = bx + bw + 10;
    ctx.font = L.BAR.LABEL_FONT;
    ctx.fillStyle = passes ? L.COLORS.pass : L.COLORS.fail;
    ctx.textAlign = 'left';
    ctx.textBaseline = 'middle';
    ctx.fillText(passes ? '✓ safe' : '✗ risky', indicatorX, by + bh / 2);
  }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Initialize the trust threshold interactive diagram.
 *
 * @param canvas           The <canvas> element to draw on.
 * @param thresholdSlider  The range input for the threshold k.
 * @param thresholdDisplay The element showing the current k value.
 * @param resultEl         The element showing the pass/fail summary.
 *
 * Gracefully skips rendering if the canvas 2D context cannot be obtained.
 * Wraps draw calls in try/catch (Req 3.3, 22.6).
 */
export function initTrustThreshold(
  canvas: HTMLCanvasElement,
  thresholdSlider: HTMLInputElement,
  thresholdDisplay: HTMLElement,
  resultEl: HTMLElement,
): void {
  const ctx = canvas.getContext('2d');
  if (!ctx) {
    console.warn('[trust-threshold] canvas context unavailable');
    return;
  }

  function getThreshold(): number {
    return parseFloat(thresholdSlider.value) / 10;
  }

  function updateSummary(k: number): void {
    const passing = EXAMPLE_DEALS.filter(d => passesSafetyCheck(d.stake, d.dealValue, k));
    const total = EXAMPLE_DEALS.length;
    resultEl.textContent = `${passing.length} of ${total} deals pass the safety check at k = ${k.toFixed(1)}`;
  }

  function render(): void {
    const k = getThreshold();
    thresholdDisplay.textContent = k.toFixed(1);
    try {
      drawThresholdDiagram(ctx!, canvas.width, canvas.height, k);
    } catch (err) {
      console.error('[trust-threshold] draw error:', err);
    }
    updateSummary(k);
  }

  thresholdSlider.addEventListener('input', render);

  // Initial render
  render();
}
