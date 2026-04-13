/**
 * Profit chart canvas renderer.
 * Renders the provider profit vs quality chart on the economics learn page.
 *
 * Mathematical model (two-leg settlement):
 *   Provider expected payoff:  E[π_P] = b_s + q·f_s - c
 *   Requester expected payoff: E[π_R] = q·v - b_s - q·f_s
 *
 * Break-even quality threshold:
 *   When f_s > 0:  q* = max(0, (c - b_s) / f_s)
 *   When f_s = 0 and b_s < c:  "never" (provider always loses)
 *   When f_s = 0 and b_s >= c:  q* = 0 (always profitable)
 */

export interface ProfitChartConfig {
  canvas: HTMLCanvasElement;
  baseFeeSlider: HTMLInputElement;
  successFeeSlider: HTMLInputElement;
  costSlider: HTMLInputElement;
  baseFeeDisplay: HTMLElement;
  successFeeDisplay: HTMLElement;
  costDisplay: HTMLElement;
  profitAtZeroDisplay: HTMLElement;
  profitAtOneDisplay: HTMLElement;
  breakEvenDisplay: HTMLElement;
  requesterValue: number;
}

/** Layout constants for the profit chart canvas. */
export const PROFIT_CHART_LAYOUT = {
  PADDING: 20,
  GRID_LINE_WIDTH: 0.5,
  ZERO_LINE_WIDTH: 1,
  PLOT_LINE_WIDTH: 2,
  QUALITY_STEPS: 100,
  FONTS: {
    tick: '9px system-ui',
    legend: '10px system-ui',
  },
  COLORS: {
    grid: '#1a1f2b',
    zeroLine: '#30363d',
    provider: '#4ade80',
    requester: '#93c5fd',
    incentive: '#fbbf24',
    tickLabel: '#4b5563',
  },
} as const;

/**
 * Compute provider payoff at quality q: baseFee + q * successFee - cost.
 * Matches formal model E[π_P] = b_s + q·f_s - c.
 */
export function computeProviderPayoff(
  baseFee: number,
  successFee: number,
  cost: number,
  q: number,
): number {
  return baseFee + q * successFee - cost;
}

/**
 * Compute requester payoff at quality q: q * value - baseFee - q * successFee.
 * Matches formal model E[π_R] = q·v - b_s - q·f_s.
 */
export function computeRequesterPayoff(
  value: number,
  baseFee: number,
  successFee: number,
  q: number,
): number {
  return q * value - baseFee - q * successFee;
}

/**
 * Compute break-even quality threshold.
 * Returns max(0, (cost - baseFee) / successFee) when successFee > 0.
 * Returns Infinity when successFee === 0 and baseFee < cost ("never" profitable).
 * Returns 0 when successFee === 0 and baseFee >= cost (always profitable).
 */
export function computeBreakEven(
  baseFee: number,
  successFee: number,
  cost: number,
): number {
  if (successFee > 0) {
    return Math.max(0, (cost - baseFee) / successFee);
  }
  return baseFee < cost ? Infinity : 0;
}

/**
 * Initialize the profit chart with slider bindings and initial render.
 * Gracefully skips rendering if the canvas 2D context cannot be obtained.
 */
export function initProfitChart(config: ProfitChartConfig): void {
  const {
    canvas,
    baseFeeSlider,
    successFeeSlider,
    costSlider,
    baseFeeDisplay,
    successFeeDisplay,
    costDisplay,
    profitAtZeroDisplay,
    profitAtOneDisplay,
    breakEvenDisplay,
    requesterValue,
  } = config;

  const ctx = canvas.getContext('2d');
  if (!ctx) {
    console.warn('[profit-chart] canvas context unavailable');
    return;
  }

  const L = PROFIT_CHART_LAYOUT;
  const v = requesterValue;

  function draw(): void {
    try {
      const bf = +baseFeeSlider.value;
      const sf = +successFeeSlider.value;
      const cost = +costSlider.value;

      // Update display values
      baseFeeDisplay.textContent = String(bf);
      successFeeDisplay.textContent = String(sf);
      costDisplay.textContent = String(cost);

      // Compute metrics
      const zeroProfit = computeProviderPayoff(bf, sf, cost, 0);
      const fullProfit = computeProviderPayoff(bf, sf, cost, 1);
      const threshold = computeBreakEven(bf, sf, cost);

      profitAtZeroDisplay.textContent = zeroProfit.toFixed(1) + ' sat';
      profitAtOneDisplay.textContent = fullProfit.toFixed(1) + ' sat';
      breakEvenDisplay.textContent = Number.isFinite(threshold)
        ? threshold.toFixed(2)
        : 'never';

      const w = canvas.width;
      const h = canvas.height;
      const pad = L.PADDING;
      const ph = h - 2 * pad;

      // Clear canvas
      ctx.clearRect(0, 0, w, h);

      // Horizontal grid lines
      ctx.strokeStyle = L.COLORS.grid;
      ctx.lineWidth = L.GRID_LINE_WIDTH;
      for (let y = pad; y <= h - pad; y += ph / 4) {
        ctx.beginPath();
        ctx.moveTo(pad, y);
        ctx.lineTo(w - pad, y);
        ctx.stroke();
      }

      // Zero line at vertical center
      const zeroY = pad + ph / 2;
      ctx.strokeStyle = L.COLORS.zeroLine;
      ctx.lineWidth = L.ZERO_LINE_WIDTH;
      ctx.beginPath();
      ctx.moveTo(pad, zeroY);
      ctx.lineTo(w - pad, zeroY);
      ctx.stroke();

      // Axis labels
      ctx.font = L.FONTS.tick;
      ctx.fillStyle = L.COLORS.tickLabel;
      ctx.textAlign = 'center';
      ctx.fillText('quality (q) \u2192', w / 2, h - 2);
      ctx.textAlign = 'left';
      ctx.fillText('profit', 2, pad - 4);
      ctx.fillText('0', pad - 14, zeroY + 3);
      ctx.fillText('1.0', w - pad + 2, h - pad + 3);
      ctx.fillText('0.0', pad - 2, h - pad + 3);

      // Scale: max absolute value determines Y range
      const maxVal = Math.max(bf + sf, v, 10);
      function yOf(val: number): number {
        return zeroY - val * (ph / 2) / maxVal;
      }

      // Provider profit line: E[π_P] = bf + q*sf - cost
      ctx.beginPath();
      for (let i = 0; i <= L.QUALITY_STEPS; i++) {
        const q = i / L.QUALITY_STEPS;
        const x = pad + i * (w - 2 * pad) / L.QUALITY_STEPS;
        const profit = computeProviderPayoff(bf, sf, cost, q);
        const y = yOf(profit);
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.strokeStyle = L.COLORS.provider;
      ctx.lineWidth = L.PLOT_LINE_WIDTH;
      ctx.stroke();

      // Requester payoff line: E[π_R] = q*v - bf - q*sf
      ctx.beginPath();
      for (let i = 0; i <= L.QUALITY_STEPS; i++) {
        const q = i / L.QUALITY_STEPS;
        const x = pad + i * (w - 2 * pad) / L.QUALITY_STEPS;
        const payoff = computeRequesterPayoff(v, bf, sf, q);
        const y = yOf(payoff);
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.strokeStyle = L.COLORS.requester;
      ctx.lineWidth = L.PLOT_LINE_WIDTH;
      ctx.stroke();

      // Legend
      ctx.font = L.FONTS.legend;
      ctx.fillStyle = L.COLORS.provider;
      ctx.textAlign = 'left';
      ctx.fillText('provider profit', pad + 8, pad + 12);
      ctx.fillStyle = L.COLORS.requester;
      ctx.fillText('requester payoff (v=' + v + ')', pad + 8, pad + 24);
      ctx.fillStyle = L.COLORS.incentive;
      ctx.fillText('slope = success_fee = ' + sf + ' (incentive strength)', pad + 8, pad + 36);
    } catch (err) {
      console.error('[profit-chart] draw error:', err);
    }
  }

  baseFeeSlider.addEventListener('input', draw);
  successFeeSlider.addEventListener('input', draw);
  costSlider.addEventListener('input', draw);
  draw();
}
