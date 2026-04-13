/**
 * Settlement outcome canvas renderer.
 * Renders the three settlement scenarios on the settlement learn page.
 *
 * Mathematical model (two-leg settlement):
 *   Success:  requester outflow = B + F,  provider inflow = B + F
 *   Failure:  requester outflow = B,      provider inflow = B  (success fee canceled)
 *   Free:     requester outflow = 0,      provider inflow = 0
 *
 * Where B = base fee, F = success fee.
 */

export interface SettlementScenario {
  title: string;
  base: {
    from: string;
    to: string;
    amount: string;
    state: string;
    color: string;
  };
  success: {
    from: string;
    to: string;
    amount: string;
    state: string;
    color: string;
  };
  requesterTotal: string;
  providerTotal: string;
  rule: string;
  note: string;
}

export interface SettlementVizConfig {
  canvas: HTMLCanvasElement;
  buttons: HTMLElement[];
  requesterTotalEl: HTMLElement;
  providerTotalEl: HTMLElement;
  ruleEl: HTMLElement;
}

/** Layout constants for the settlement viz canvas. */
export const SETTLEMENT_VIZ_LAYOUT = {
  TITLE_Y: 20,
  BASE_FEE_Y: 55,
  SUCCESS_FEE_Y: 90,
  NOTE_OFFSET_BOTTOM: 16,
  LABEL_X: 20,
  AMOUNT_X: 120,
  STATE_X: 240,
  ARROW_START_X: 310,
  ARROW_END_X: 380,
  ARROW_HEAD_SIZE: 4,
  FROM_LABEL_X: 390,
  ARROW_LINE_WIDTH: 1.5,
  ARROW_ALPHA_SUFFIX: '88',
  FONTS: {
    title: 'bold 12px system-ui',
    label: '10px system-ui',
    fromLabel: '9px system-ui',
    note: '11px system-ui',
  },
  COLORS: {
    title: '#e5e7eb',
    label: '#9ca3af',
    state: '#6b7280',
    settled: '#4ade80',
    canceled: '#f87171',
    warning: '#fbbf24',
    free: '#818cf8',
    note: '#9ca3af',
  },
} as const;

/**
 * Compute requester outflow for a given scenario.
 * Success: B + F, Failure: B, Free: 0
 */
export function computeRequesterOutflow(
  baseFee: number,
  successFee: number,
  scenario: 'success' | 'failure' | 'free',
): number {
  switch (scenario) {
    case 'success':
      return baseFee + successFee;
    case 'failure':
      return baseFee;
    case 'free':
      return 0;
  }
}

/**
 * Compute provider inflow for a given scenario.
 * Success: B + F, Failure: B, Free: 0
 */
export function computeProviderInflow(
  baseFee: number,
  successFee: number,
  scenario: 'success' | 'failure' | 'free',
): number {
  switch (scenario) {
    case 'success':
      return baseFee + successFee;
    case 'failure':
      return baseFee;
    case 'free':
      return 0;
  }
}

/**
 * Format a msat value for display (e.g. "8,000 msat").
 */
export function formatMsat(value: number): string {
  if (value === 0) return '0';
  return value.toLocaleString('en-US') + ' msat';
}

/**
 * Build the three settlement scenarios from base fee and success fee values.
 * Ensures mathematical consistency:
 *   - Success: requester outflow = B + F, provider inflow = B + F (Req 18.1)
 *   - Failure: requester outflow = B, provider inflow = B, success fee canceled (Req 18.2)
 *   - Free: both zero (Req 18.3)
 */
export function buildScenarios(
  baseFee: number,
  successFee: number,
): Record<string, SettlementScenario> {
  const L = SETTLEMENT_VIZ_LAYOUT;
  const total = baseFee + successFee;

  return {
    ok: {
      title: 'Successful execution',
      base: {
        from: 'Requester',
        to: 'Provider',
        amount: formatMsat(baseFee),
        state: 'settled',
        color: L.COLORS.settled,
      },
      success: {
        from: 'Requester',
        to: 'Provider',
        amount: formatMsat(successFee),
        state: 'settled',
        color: L.COLORS.settled,
      },
      requesterTotal: formatMsat(total),
      providerTotal: formatMsat(total),
      rule: 'Both legs settle',
      note: `Both fees settled. Provider earned ${total.toLocaleString('en-US')} msat. Requester got result.`,
    },
    fail: {
      title: 'Provider failed',
      base: {
        from: 'Requester',
        to: 'Provider',
        amount: formatMsat(baseFee),
        state: 'settled',
        color: L.COLORS.warning,
      },
      success: {
        from: 'Requester',
        to: '\u2014',
        amount: formatMsat(successFee),
        state: 'canceled',
        color: L.COLORS.canceled,
      },
      requesterTotal: formatMsat(baseFee),
      providerTotal: formatMsat(baseFee),
      rule: 'Base only',
      note: 'Base fee paid (provider reserved resources). Success fee returned to requester.',
    },
    free: {
      title: 'Free service (settlement: none)',
      base: {
        from: '\u2014',
        to: '\u2014',
        amount: '0',
        state: 'n/a',
        color: L.COLORS.free,
      },
      success: {
        from: '\u2014',
        to: '\u2014',
        amount: '0',
        state: 'n/a',
        color: L.COLORS.free,
      },
      requesterTotal: '0',
      providerTotal: '0',
      rule: 'No value transfer',
      note: 'No payment. Deal flow still runs. Quote, deal, execute, receipt \u2014 just no money.',
    },
  };
}


/**
 * Draw a single fee row (base or success) on the canvas.
 */
function drawFeeRow(
  ctx: CanvasRenderingContext2D,
  y: number,
  label: string,
  fee: SettlementScenario['base'],
): void {
  const L = SETTLEMENT_VIZ_LAYOUT;

  // Label
  ctx.font = L.FONTS.label;
  ctx.fillStyle = L.COLORS.label;
  ctx.textAlign = 'left';
  ctx.fillText(label, L.LABEL_X, y);

  // Amount
  ctx.fillStyle = fee.color;
  ctx.fillText(fee.amount, L.AMOUNT_X, y);

  // State
  ctx.fillStyle = L.COLORS.state;
  ctx.fillText(fee.state, L.STATE_X, y);

  // Arrow
  const arrowY = y - 4;
  ctx.beginPath();
  ctx.moveTo(L.ARROW_START_X, arrowY);
  ctx.lineTo(L.ARROW_END_X, arrowY);
  ctx.strokeStyle = fee.color + L.ARROW_ALPHA_SUFFIX;
  ctx.lineWidth = L.ARROW_LINE_WIDTH;
  ctx.stroke();

  // Arrow head
  const hs = L.ARROW_HEAD_SIZE;
  ctx.fillStyle = fee.color + L.ARROW_ALPHA_SUFFIX;
  ctx.beginPath();
  ctx.moveTo(L.ARROW_END_X, arrowY);
  ctx.lineTo(L.ARROW_END_X - hs * 1.5, arrowY - hs);
  ctx.lineTo(L.ARROW_END_X - hs * 1.5, arrowY + hs);
  ctx.fill();

  // From label
  ctx.font = L.FONTS.fromLabel;
  ctx.fillStyle = L.COLORS.state;
  ctx.textAlign = 'left';
  ctx.fillText(fee.from, L.FROM_LABEL_X, y);
}

/**
 * Draw a complete scenario on the canvas.
 */
function drawScenario(
  ctx: CanvasRenderingContext2D,
  scenario: SettlementScenario,
  w: number,
  h: number,
): void {
  const L = SETTLEMENT_VIZ_LAYOUT;

  ctx.clearRect(0, 0, w, h);

  // Title
  ctx.font = L.FONTS.title;
  ctx.fillStyle = L.COLORS.title;
  ctx.textAlign = 'center';
  ctx.fillText(scenario.title, w / 2, L.TITLE_Y);

  // Base fee row
  drawFeeRow(ctx, L.BASE_FEE_Y, 'base fee', scenario.base);

  // Success fee row
  drawFeeRow(ctx, L.SUCCESS_FEE_Y, 'success fee', scenario.success);

  // Note
  ctx.font = L.FONTS.note;
  ctx.fillStyle = L.COLORS.note;
  ctx.textAlign = 'center';
  ctx.fillText(scenario.note, w / 2, h - L.NOTE_OFFSET_BOTTOM);
}

/**
 * Initialize the settlement visualization with button bindings and initial render.
 * Gracefully skips rendering if the canvas 2D context cannot be obtained.
 *
 * Default fee values match the inline example: base = 3000 msat, success = 5000 msat.
 */
export function initSettlementViz(
  config: SettlementVizConfig,
  baseFee = 3000,
  successFee = 5000,
): void {
  const { canvas, buttons, requesterTotalEl, providerTotalEl, ruleEl } = config;

  const ctx = canvas.getContext('2d');
  if (!ctx) {
    console.warn('[settle-viz] canvas context unavailable');
    return;
  }

  const scenarios = buildScenarios(baseFee, successFee);

  function showScenario(key: string): void {
    const s = scenarios[key];
    if (!s) return;

    // Update button active states
    buttons.forEach((btn) => {
      const el = btn as HTMLElement;
      el.classList.toggle(
        'is-active',
        (el as HTMLElement).dataset.scenario === key,
      );
    });

    // Update metric displays
    requesterTotalEl.textContent = s.requesterTotal;
    providerTotalEl.textContent = s.providerTotal;
    ruleEl.textContent = s.rule;

    try {
      drawScenario(ctx, s, canvas.width, canvas.height);
    } catch (err) {
      console.error('[settle-viz] draw error:', err);
    }
  }

  // Bind button clicks
  buttons.forEach((btn) => {
    btn.addEventListener('click', () => {
      const key = (btn as HTMLElement).dataset.scenario;
      if (key) showScenario(key);
    });
  });

  // Initial render
  showScenario('ok');
}
