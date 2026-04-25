/**
 * Interactive trust threshold diagram.
 *
 * The user sets a risk threshold k and sees which example
 * stake-to-deal-value ratios pass `stake / deal_value > k`.
 *
 * Rendering rules enforced here:
 *   - Canvas is sized in CSS pixels with a separate backing-store scaled to
 *     devicePixelRatio so text stays crisp on retina displays.
 *   - Canvas height is computed from the bar count so bars never overflow.
 *   - A right-side gutter is reserved for the pass/fail indicator so it never
 *     clips the canvas or overlaps interior text.
 *   - The threshold label is clamped horizontally so it stays inside the
 *     drawable area even at very small or very large k.
 *   - A ResizeObserver re-renders the canvas when its container changes width.
 */

// ---------------------------------------------------------------------------
// Layout constants (CSS pixels)
// ---------------------------------------------------------------------------

export const THRESHOLD_LAYOUT = {
  PADDING: {
    top: 32,        // headroom for the "k = X.X" label above bars
    right: 78,      // gutter reserved for the "✓ safe" / "✗ risky" indicator
    bottom: 16,
    left: 16,
  },
  BAR: {
    HEIGHT: 34,
    RADIUS: 6,
    GAP: 10,
    LABEL_FONT: "600 12px 'Inter', system-ui, sans-serif",
    VALUE_FONT: "11px 'JetBrains Mono', ui-monospace, Menlo, monospace",
    /** Vertical offset of the deal label from the bar's vertical centre. */
    LABEL_OFFSET: -7,
    /** Vertical offset of the ratio text from the bar's vertical centre. */
    VALUE_OFFSET: 8,
    /** Horizontal padding for text inside the bar. */
    TEXT_INSET: 12,
    /** Horizontal gap between the end of the bar and the indicator. */
    INDICATOR_GAP: 10,
  },
  LINE: {
    WIDTH: 1.5,
    DASH: [6, 4],
    COLOR: '#f5c518',
    LABEL_FONT: "bold 11px 'JetBrains Mono', ui-monospace, Menlo, monospace",
  },
  // Canvas can't read CSS vars, so keep these in sync with --frog-400 / --danger / --fg1 / --fg3.
  COLORS: {
    pass: '#52c72a',
    fail: '#e54848',
    passFill: 'rgba(82,199,42,0.16)',
    failFill: 'rgba(229,72,72,0.16)',
    passStroke: 'rgba(82,199,42,0.55)',
    failStroke: 'rgba(229,72,72,0.55)',
    labelText: '#e8ede6',
    valueText: '#9aa497',
  },
  /** Minimum visible width of the smallest bar. */
  MIN_BAR_WIDTH: 14,
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

export function computeRatio(stake: number, dealValue: number): number {
  if (dealValue === 0) return Infinity;
  return stake / dealValue;
}

export function passesSafetyCheck(stake: number, dealValue: number, k: number): boolean {
  return computeRatio(stake, dealValue) > k;
}

/**
 * Total CSS-pixel height required to render all bars without overflow,
 * given the current layout constants.
 */
export function computeCanvasHeight(barCount: number = EXAMPLE_DEALS.length): number {
  const L = THRESHOLD_LAYOUT;
  const barTotal = barCount * L.BAR.HEIGHT + Math.max(0, barCount - 1) * L.BAR.GAP;
  return L.PADDING.top + barTotal + L.PADDING.bottom;
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

function roundedRectPath(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
): void {
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

function drawThresholdDiagram(
  ctx: CanvasRenderingContext2D,
  cssW: number,
  cssH: number,
  threshold: number,
): void {
  const L = THRESHOLD_LAYOUT;
  const deals = EXAMPLE_DEALS;
  const n = deals.length;

  ctx.clearRect(0, 0, cssW, cssH);

  const ratios = deals.map(d => computeRatio(d.stake, d.dealValue));
  const maxRatioInData = Math.max(...ratios.filter(Number.isFinite), 0);
  const maxRatio = Math.max(maxRatioInData, threshold * 1.3, 2);

  const usableW = Math.max(1, cssW - L.PADDING.left - L.PADDING.right);
  const startY = L.PADDING.top;

  // --- Threshold vertical line (drawn first, behind bars) ---
  const threshX = L.PADDING.left + (threshold / maxRatio) * usableW;
  ctx.save();
  ctx.setLineDash([...L.LINE.DASH]);
  ctx.strokeStyle = L.LINE.COLOR;
  ctx.lineWidth = L.LINE.WIDTH;
  ctx.beginPath();
  ctx.moveTo(threshX, L.PADDING.top - 6);
  ctx.lineTo(threshX, cssH - L.PADDING.bottom + 4);
  ctx.stroke();
  ctx.restore();

  // --- Threshold label (clamped to canvas) ---
  ctx.font = L.LINE.LABEL_FONT;
  ctx.fillStyle = L.LINE.COLOR;
  ctx.textBaseline = 'alphabetic';
  const labelText = `k = ${threshold.toFixed(1)}`;
  const labelW = ctx.measureText(labelText).width;
  const labelHalf = labelW / 2 + 4;
  const minLabelX = L.PADDING.left + labelHalf;
  const maxLabelX = cssW - L.PADDING.right + labelHalf; // allow label to extend slightly into right gutter
  const labelX = Math.max(minLabelX, Math.min(threshX, maxLabelX));
  ctx.textAlign = 'center';
  ctx.fillText(labelText, labelX, L.PADDING.top - 12);

  // --- Bars ---
  for (let i = 0; i < n; i++) {
    const deal = deals[i];
    const ratio = ratios[i];
    const passes = passesSafetyCheck(deal.stake, deal.dealValue, threshold);
    const y = startY + i * (L.BAR.HEIGHT + L.BAR.GAP);

    const proportional = (ratio / maxRatio) * usableW;
    const barW = Math.max(L.MIN_BAR_WIDTH, Math.min(proportional, usableW));
    const bx = L.PADDING.left;
    const by = y;
    const bh = L.BAR.HEIGHT;

    roundedRectPath(ctx, bx, by, barW, bh, L.BAR.RADIUS);
    ctx.fillStyle = passes ? L.COLORS.passFill : L.COLORS.failFill;
    ctx.fill();
    ctx.strokeStyle = passes ? L.COLORS.passStroke : L.COLORS.failStroke;
    ctx.lineWidth = 1.2;
    ctx.stroke();

    // Pass/fail indicator anchor — fixed in the right gutter so it never
    // clips the canvas and never overlaps the bar's text, regardless of width.
    const indicatorX = cssW - L.PADDING.right + L.BAR.INDICATOR_GAP;

    // Choose where to render the deal label + ratio: inside the bar when it's
    // wide enough to read, otherwise in the empty space to the right of the
    // bar (still left of the indicator). The threshold accounts for the
    // longest ratio string ("ratio: 10.0  (5000/4000)") in the value font.
    const interiorTextWidth = barW - 2 * L.BAR.TEXT_INSET;
    const renderInside = interiorTextWidth > 150;

    if (renderInside) {
      ctx.save();
      roundedRectPath(ctx, bx, by, barW, bh, L.BAR.RADIUS);
      ctx.clip();

      ctx.font = L.BAR.LABEL_FONT;
      ctx.fillStyle = L.COLORS.labelText;
      ctx.textAlign = 'left';
      ctx.textBaseline = 'middle';
      ctx.fillText(deal.label, bx + L.BAR.TEXT_INSET, by + bh / 2 + L.BAR.LABEL_OFFSET);

      ctx.font = L.BAR.VALUE_FONT;
      ctx.fillStyle = L.COLORS.valueText;
      ctx.fillText(
        `ratio: ${ratio.toFixed(1)}  (${deal.stake}/${deal.dealValue})`,
        bx + L.BAR.TEXT_INSET,
        by + bh / 2 + L.BAR.VALUE_OFFSET,
      );
      ctx.restore();
    } else {
      // Render label after the bar but before the indicator gutter.
      const textX = bx + barW + 10;
      ctx.font = L.BAR.LABEL_FONT;
      ctx.fillStyle = L.COLORS.labelText;
      ctx.textAlign = 'left';
      ctx.textBaseline = 'middle';
      ctx.fillText(deal.label, textX, by + bh / 2 + L.BAR.LABEL_OFFSET);

      ctx.font = L.BAR.VALUE_FONT;
      ctx.fillStyle = L.COLORS.valueText;
      ctx.fillText(
        `${ratio.toFixed(1)}  (${deal.stake}/${deal.dealValue})`,
        textX,
        by + bh / 2 + L.BAR.VALUE_OFFSET,
      );
    }

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

  // Lock the CSS-pixel height to the bar layout so bars always fit.
  const cssH = computeCanvasHeight();
  canvas.style.height = `${cssH}px`;
  canvas.style.display = 'block';
  canvas.style.width = canvas.style.width || '100%';

  function getThreshold(): number {
    return parseFloat(thresholdSlider.value) / 10;
  }

  function updateSummary(k: number): void {
    const passing = EXAMPLE_DEALS.filter(d => passesSafetyCheck(d.stake, d.dealValue, k));
    const total = EXAMPLE_DEALS.length;
    resultEl.textContent =
      `${passing.length} of ${total} deals pass the safety check at k = ${k.toFixed(1)}`;
  }

  function syncBackingStore(): { w: number; h: number; dpr: number } {
    const dpr = Math.max(1, Math.min(3, window.devicePixelRatio || 1));
    const rect = canvas.getBoundingClientRect();
    const cssW = Math.max(280, Math.round(rect.width));
    const targetW = Math.round(cssW * dpr);
    const targetH = Math.round(cssH * dpr);
    if (canvas.width !== targetW || canvas.height !== targetH) {
      canvas.width = targetW;
      canvas.height = targetH;
    }
    // Reset to identity then apply DPR — avoids accumulating transforms
    // across multiple renders.
    ctx!.setTransform(dpr, 0, 0, dpr, 0, 0);
    return { w: cssW, h: cssH, dpr };
  }

  function render(): void {
    const k = getThreshold();
    thresholdDisplay.textContent = k.toFixed(1);
    try {
      const { w, h } = syncBackingStore();
      drawThresholdDiagram(ctx!, w, h, k);
    } catch (err) {
      console.error('[trust-threshold] draw error:', err);
    }
    updateSummary(k);
  }

  thresholdSlider.addEventListener('input', render);

  // Re-render on container resize (responsive layouts) and DPR changes
  // (window moved between displays).
  if (typeof ResizeObserver !== 'undefined') {
    const ro = new ResizeObserver(() => render());
    ro.observe(canvas);
  } else {
    window.addEventListener('resize', render);
  }

  // Initial render
  render();
}
