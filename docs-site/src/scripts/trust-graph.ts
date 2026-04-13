/**
 * Trust graph canvas renderer.
 * Renders the stake-vs-fee cheat payoff chart on the landing page.
 *
 * Mathematical model:
 *   cheatPayoff = fee - stake  (provider keeps fee, loses stake)
 *   ratio       = stake / fee
 *   payoff(r)   = fee × (1 - r)  where r = stake / fee
 *
 * When ratio = 1.0 → payoff is exactly zero
 * When ratio > 1.0 → payoff is negative (cheating irrational)
 * When ratio < 1.0 → payoff is positive (cheating profitable)
 */

export interface TrustGraphConfig {
  canvas: HTMLCanvasElement;
  stakeSlider: HTMLInputElement;
  feeSlider: HTMLInputElement;
  stakeDisplay: HTMLElement;
  feeDisplay: HTMLElement;
}

/** Layout constants replacing all magic numbers in the canvas drawing code. */
export const TRUST_GRAPH_LAYOUT = {
  PADDING: { top: 24, right: 20, bottom: 40, left: 50 },
  MAX_RATIO: 5,
  Y_MAX_FACTOR: 1.3,
  Y_MIN_FACTOR: 2,
  GRID_LINE_WIDTH: 0.5,
  ZERO_LINE_WIDTH: 1,
  CHEAT_LINE_WIDTH: 2.5,
  MARKER_LINE_WIDTH: 1,
  MARKER_DASH: [4, 4] as readonly number[],
  ZERO_DOT_RADIUS: 5,
  CURRENT_DOT_RADIUS: 6,
  DOT_STROKE_WIDTH: 2,
  CURRENT_DASH_LINE_WIDTH: 1,
  CURRENT_DASH: [4, 4] as readonly number[],
  Y_STEP_THRESHOLDS: [
    { above: 2000, step: 500 },
    { above: 800, step: 200 },
    { above: 400, step: 100 },
  ] as readonly { above: number; step: number }[],
  Y_STEP_DEFAULT: 50,
  TICK_OFFSET_X: 15,
  TICK_OFFSET_Y: 3,
  TICK_LABEL_MARGIN: 6,
  AXIS_LABEL_OFFSET_Y: 6,
  Y_AXIS_TRANSLATE_X: 12,
  STAKE_EQ_FEE_LABEL_OFFSET_X: 8,
  STAKE_EQ_FEE_LABEL_OFFSET_Y: 8,
  CHEAT_ZONE_LABEL_RATIO: 0.5,
  CHEAT_ZONE_LABEL_Y_FACTOR: 0.6,
  SAFE_ZONE_LABEL_RATIO: 3,
  SAFE_ZONE_LABEL_Y_FACTOR: 0.35,
  LINE_LABEL_MARGIN: 4,
  LINE_LABEL_OFFSET_Y: 8,
  PAYOFF_LABEL_OFFSET_X: 10,
  PAYOFF_LABEL_OFFSET_Y: 4,
  RATIO_LABEL_OFFSET_Y: 6,
  LARGE_NUMBER_THRESHOLD: 1000,
  FONTS: {
    axisLabel: '11px Inter, system-ui, sans-serif',
    tick: '10px JetBrains Mono, monospace',
    zoneLabel: 'bold 11px Inter, system-ui, sans-serif',
    ratioLabel: 'bold 10px Inter, system-ui, sans-serif',
    payoffLabel: '10px JetBrains Mono, monospace',
  },
  COLORS: {
    background: '#0d1117',
    grid: '#1a2332',
    zeroLine: '#2d3748',
    cheatLine: '#f87171',
    safeZone: 'rgba(74, 222, 128, 0.06)',
    marker: '#fbbf24',
    markerDash: '#fbbf2444',
    currentSafe: '#4ade80',
    currentDanger: '#f87171',
    currentDash: '#67e8f966',
    ratioText: '#67e8f9',
    stakeEqFee: '#fbbf24',
    cheatZoneText: 'rgba(248, 113, 113, 0.35)',
    safeZoneText: 'rgba(74, 222, 128, 0.4)',
    axisLabel: '#6b7280',
    tickLabel: '#4b5563',
    backgroundStroke: '#0d1117',
  },
} as const;

/**
 * Compute cheat payoff: fee - stake.
 * The provider keeps the fee but loses the stake.
 */
export function computeCheatPayoff(stake: number, fee: number): number {
  return fee - stake;
}

/**
 * Compute stake-to-fee ratio: stake / fee.
 */
export function computeRatio(stake: number, fee: number): number {
  return stake / fee;
}

/** Determine the Y-axis grid step based on the Y range. */
function computeYStep(yRange: number): number {
  for (const { above, step } of TRUST_GRAPH_LAYOUT.Y_STEP_THRESHOLDS) {
    if (yRange > above) return step;
  }
  return TRUST_GRAPH_LAYOUT.Y_STEP_DEFAULT;
}

/** Format a tick value for the Y axis. */
function formatTick(value: number): string {
  if (value === 0) return '0';
  const prefix = value > 0 ? '+' : '';
  if (Math.abs(value) >= TRUST_GRAPH_LAYOUT.LARGE_NUMBER_THRESHOLD) {
    return prefix + (value / 1000).toFixed(1) + 'k';
  }
  return prefix + value;
}

/**
 * Initialize the trust graph with slider bindings and initial render.
 * Gracefully skips rendering if the canvas 2D context cannot be obtained.
 */
export function initTrustGraph(config: TrustGraphConfig): void {
  const { canvas, stakeSlider, feeSlider, stakeDisplay, feeDisplay } = config;
  const ctx = canvas.getContext('2d');
  if (!ctx) return;

  const L = TRUST_GRAPH_LAYOUT;

  function draw(): void {
    try {
      const stake = +stakeSlider.value;
      const fee = +feeSlider.value;

      stakeDisplay.textContent = String(stake);
      feeDisplay.textContent = String(fee);

      const ratio = computeRatio(stake, fee);
      const cheatPayoff = computeCheatPayoff(stake, fee);

      const W = canvas.width;
      const H = canvas.height;
      const pad = L.PADDING;
      const pw = W - pad.left - pad.right;
      const ph = H - pad.top - pad.bottom;

      const maxR = L.MAX_RATIO;
      const xr = (r: number) => pad.left + (r / maxR) * pw;

      const yMax = fee * L.Y_MAX_FACTOR;
      const yMin = -fee * L.Y_MIN_FACTOR;
      const yRange = yMax - yMin;
      const yn = (v: number) => pad.top + (1 - (v - yMin) / yRange) * ph;

      const yStep = computeYStep(yRange);

      // Clear
      ctx.clearRect(0, 0, W, H);
      ctx.fillStyle = L.COLORS.background;
      ctx.fillRect(0, 0, W, H);

      // Grid lines
      ctx.strokeStyle = L.COLORS.grid;
      ctx.lineWidth = L.GRID_LINE_WIDTH;
      for (let gy = Math.ceil(yMin / yStep) * yStep; gy <= yMax; gy += yStep) {
        ctx.beginPath();
        ctx.moveTo(pad.left, yn(gy));
        ctx.lineTo(W - pad.right, yn(gy));
        ctx.stroke();
      }
      for (let gx = 0; gx <= maxR; gx++) {
        ctx.beginPath();
        ctx.moveTo(xr(gx), pad.top);
        ctx.lineTo(xr(gx), H - pad.bottom);
        ctx.stroke();
      }

      // Zero line
      ctx.strokeStyle = L.COLORS.zeroLine;
      ctx.lineWidth = L.ZERO_LINE_WIDTH;
      ctx.beginPath();
      ctx.moveTo(pad.left, yn(0));
      ctx.lineTo(W - pad.right, yn(0));
      ctx.stroke();

      // Safe zone fill (ratio > 1: cheating costs money)
      ctx.fillStyle = L.COLORS.safeZone;
      ctx.beginPath();
      ctx.moveTo(xr(1), yn(0));
      ctx.lineTo(xr(maxR), yn(0));
      ctx.lineTo(xr(maxR), yn(fee * (1 - maxR)));
      ctx.lineTo(xr(1), yn(0));
      ctx.closePath();
      ctx.fill();

      // Cheat payoff line: payoff(r) = fee × (1 - r)
      ctx.beginPath();
      ctx.moveTo(xr(0), yn(fee));
      ctx.lineTo(xr(maxR), yn(fee * (1 - maxR)));
      ctx.strokeStyle = L.COLORS.cheatLine;
      ctx.lineWidth = L.CHEAT_LINE_WIDTH;
      ctx.stroke();

      // stake = fee marker at ratio 1
      ctx.setLineDash([...L.MARKER_DASH]);
      ctx.strokeStyle = L.COLORS.markerDash;
      ctx.lineWidth = L.MARKER_LINE_WIDTH;
      ctx.beginPath();
      ctx.moveTo(xr(1), pad.top);
      ctx.lineTo(xr(1), H - pad.bottom);
      ctx.stroke();
      ctx.setLineDash([]);

      // Zero crossing dot
      ctx.beginPath();
      ctx.arc(xr(1), yn(0), L.ZERO_DOT_RADIUS, 0, Math.PI * 2);
      ctx.fillStyle = L.COLORS.marker;
      ctx.fill();
      ctx.strokeStyle = L.COLORS.backgroundStroke;
      ctx.lineWidth = L.DOT_STROKE_WIDTH;
      ctx.stroke();

      // Current position
      if (ratio <= maxR) {
        const mx = xr(ratio);
        const my = yn(cheatPayoff);

        // Vertical guide at current ratio
        ctx.setLineDash([...L.CURRENT_DASH]);
        ctx.strokeStyle = L.COLORS.currentDash;
        ctx.lineWidth = L.CURRENT_DASH_LINE_WIDTH;
        ctx.beginPath();
        ctx.moveTo(mx, pad.top);
        ctx.lineTo(mx, H - pad.bottom);
        ctx.stroke();
        ctx.setLineDash([]);

        // Current position dot
        ctx.beginPath();
        ctx.arc(mx, my, L.CURRENT_DOT_RADIUS, 0, Math.PI * 2);
        ctx.fillStyle = cheatPayoff > 0 ? L.COLORS.currentDanger : L.COLORS.currentSafe;
        ctx.fill();
        ctx.strokeStyle = L.COLORS.backgroundStroke;
        ctx.lineWidth = L.DOT_STROKE_WIDTH;
        ctx.stroke();

        // Ratio label above
        ctx.font = L.FONTS.ratioLabel;
        ctx.fillStyle = L.COLORS.ratioText;
        ctx.textAlign = 'center';
        ctx.fillText(ratio.toFixed(1) + 'x', mx, pad.top - L.RATIO_LABEL_OFFSET_Y);

        // Numeric payoff value adjacent to dot (Req 13.2 — accessibility)
        ctx.font = L.FONTS.payoffLabel;
        ctx.fillStyle = cheatPayoff > 0 ? L.COLORS.currentDanger : L.COLORS.currentSafe;
        ctx.textAlign = 'left';
        ctx.fillText(
          (cheatPayoff > 0 ? '+' : '') + cheatPayoff + ' sats',
          mx + L.PAYOFF_LABEL_OFFSET_X,
          my + L.PAYOFF_LABEL_OFFSET_Y,
        );
      }

      // stake = fee label
      ctx.font = L.FONTS.axisLabel;
      ctx.fillStyle = L.COLORS.stakeEqFee;
      ctx.textAlign = 'left';
      ctx.fillText(
        'stake = fee',
        xr(1) + L.STAKE_EQ_FEE_LABEL_OFFSET_X,
        yn(0) - L.STAKE_EQ_FEE_LABEL_OFFSET_Y,
      );

      // Zone text labels (Req 13.1 — accessibility: differentiate zones with text, not just color)
      ctx.font = L.FONTS.zoneLabel;
      ctx.textAlign = 'center';
      ctx.fillStyle = L.COLORS.cheatZoneText;
      ctx.fillText(
        'CHEAT PROFITABLE',
        xr(L.CHEAT_ZONE_LABEL_RATIO),
        yn(fee * L.CHEAT_ZONE_LABEL_Y_FACTOR),
      );
      ctx.fillStyle = L.COLORS.safeZoneText;
      ctx.fillText(
        'CHEATING COSTS MONEY',
        xr(L.SAFE_ZONE_LABEL_RATIO),
        yn(yMin * L.SAFE_ZONE_LABEL_Y_FACTOR),
      );

      // Payoff line label
      ctx.font = L.FONTS.zoneLabel;
      ctx.textAlign = 'right';
      ctx.fillStyle = L.COLORS.cheatLine;
      ctx.fillText(
        'cheat payoff = fee \u2212 stake',
        W - pad.right - L.LINE_LABEL_MARGIN,
        yn(fee * (1 - maxR)) - L.LINE_LABEL_OFFSET_Y,
      );

      // Axis labels
      ctx.font = L.FONTS.axisLabel;
      ctx.fillStyle = L.COLORS.axisLabel;
      ctx.textAlign = 'center';
      ctx.fillText('stake / fee', pad.left + pw / 2, H - L.AXIS_LABEL_OFFSET_Y);
      ctx.save();
      ctx.translate(L.Y_AXIS_TRANSLATE_X, pad.top + ph / 2);
      ctx.rotate(-Math.PI / 2);
      ctx.fillText('cheat payoff (sats)', 0, 0);
      ctx.restore();

      // X ticks
      ctx.textAlign = 'center';
      ctx.fillStyle = L.COLORS.tickLabel;
      ctx.font = L.FONTS.tick;
      for (let t = 0; t <= maxR; t++) {
        ctx.fillText(t + 'x', xr(t), H - pad.bottom + L.TICK_OFFSET_X);
      }

      // Y ticks
      ctx.textAlign = 'right';
      for (let t = Math.ceil(yMin / yStep) * yStep; t <= yMax; t += yStep) {
        if (t === 0) {
          ctx.fillStyle = L.COLORS.axisLabel;
          ctx.fillText('0', pad.left - L.TICK_LABEL_MARGIN, yn(0) + L.TICK_OFFSET_Y);
          continue;
        }
        ctx.fillStyle = L.COLORS.tickLabel;
        ctx.fillText(formatTick(t), pad.left - L.TICK_LABEL_MARGIN, yn(t) + L.TICK_OFFSET_Y);
      }
    } catch (err) {
      console.error('[trust-graph] draw error:', err);
    }
  }

  stakeSlider.addEventListener('input', draw);
  feeSlider.addEventListener('input', draw);
  draw();
}
