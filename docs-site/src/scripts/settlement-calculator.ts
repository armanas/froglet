/**
 * Interactive settlement calculator.
 * Computes settlement outcomes from user-supplied base fee, success fee, and cost.
 *
 * Mathematical model (two-leg settlement):
 *   Success:  requester outflow = B + F,  provider inflow = B + F,  profit = B + F - C
 *   Failure:  requester outflow = B,      provider inflow = B,      profit = B - C
 *   Free:     requester outflow = 0,      provider inflow = 0,      profit = -C
 *
 * Break-even quality threshold:
 *   When f_s > 0:  q* = clamp(max(0, (C - B) / F), 0, 1)
 *   When f_s = 0 and B < C:  Infinity (never profitable)
 *   When f_s = 0 and B >= C:  0 (always profitable)
 */

export interface SettlementInputs {
  baseFee: number;    // msat
  successFee: number; // msat
  cost: number;       // msat
}

export interface SettlementOutcome {
  scenario: 'success' | 'failure' | 'free';
  requesterOutflow: number;
  providerInflow: number;
  providerProfit: number;
}

/**
 * Compute all three settlement outcomes from inputs.
 * Success: requester pays base + success, provider receives base + success, profit = base + success - cost.
 * Failure: requester pays base only, provider receives base only, profit = base - cost.
 * Free: zero transfer, profit = -cost.
 */
export function computeSettlementOutcomes(
  inputs: SettlementInputs,
): SettlementOutcome[] {
  const { baseFee, successFee, cost } = inputs;
  return [
    {
      scenario: 'success',
      requesterOutflow: baseFee + successFee,
      providerInflow: baseFee + successFee,
      providerProfit: baseFee + successFee - cost,
    },
    {
      scenario: 'failure',
      requesterOutflow: baseFee,
      providerInflow: baseFee,
      providerProfit: baseFee - cost,
    },
    {
      scenario: 'free',
      requesterOutflow: 0,
      providerInflow: 0,
      providerProfit: -cost,
    },
  ];
}

/**
 * Compute break-even quality threshold.
 * Returns max(0, (cost - baseFee) / successFee) clamped to [0, 1] when successFee > 0.
 * Returns Infinity when successFee === 0 and baseFee < cost.
 * Returns 0 when baseFee >= cost (always profitable, regardless of successFee).
 */
export function computeBreakEvenThreshold(inputs: SettlementInputs): number {
  const { baseFee, successFee, cost } = inputs;
  if (baseFee >= cost) return 0;
  if (successFee <= 0) return Infinity;
  const raw = Math.max(0, (cost - baseFee) / successFee);
  return Math.min(raw, 1);
}

/**
 * Check if provider operates at a loss even at full quality (q=1).
 * True when baseFee + successFee < cost.
 */
export function isProviderAtLoss(inputs: SettlementInputs): boolean {
  return inputs.baseFee + inputs.successFee < inputs.cost;
}

/** Layout constants for the settlement calculator widget. */
const CALC_LAYOUT = {
  INPUT_MAX: 100_000,
  INPUT_STEP: 100,
  DEFAULT_BASE_FEE: 3000,
  DEFAULT_SUCCESS_FEE: 5000,
  DEFAULT_COST: 4000,
  SCENARIO_LABELS: {
    success: 'Success',
    failure: 'Provider fails',
    free: 'Free service',
  } as Record<string, string>,
  COLORS: {
    profit: '#4ade80',
    loss: '#f87171',
    warning: '#fbbf24',
    neutral: '#9ca3af',
  },
} as const;

/**
 * Format a msat value for display.
 */
function formatMsat(value: number): string {
  if (value === 0) return '0 msat';
  return value.toLocaleString('en-US') + ' msat';
}

/**
 * Create a labeled range input control.
 */
function createInputControl(
  container: HTMLElement,
  label: string,
  id: string,
  defaultValue: number,
): HTMLInputElement {
  const wrapper = document.createElement('div');
  wrapper.className = 'calc-input-row';

  const lbl = document.createElement('label');
  lbl.htmlFor = id;
  lbl.textContent = label;

  const input = document.createElement('input');
  input.type = 'range';
  input.id = id;
  input.min = '0';
  input.max = String(CALC_LAYOUT.INPUT_MAX);
  input.step = String(CALC_LAYOUT.INPUT_STEP);
  input.value = String(defaultValue);

  const display = document.createElement('span');
  display.className = 'calc-input-value';
  display.textContent = formatMsat(defaultValue);

  input.addEventListener('input', () => {
    display.textContent = formatMsat(+input.value);
  });

  wrapper.appendChild(lbl);
  wrapper.appendChild(input);
  wrapper.appendChild(display);
  container.appendChild(wrapper);

  return input;
}

/**
 * Render outcome rows into the results container.
 */
function renderOutcomes(
  resultsEl: HTMLElement,
  outcomes: SettlementOutcome[],
  atLoss: boolean,
  threshold: number,
): void {
  let html = '';

  for (const o of outcomes) {
    const profitColor =
      o.providerProfit > 0
        ? CALC_LAYOUT.COLORS.profit
        : o.providerProfit < 0
          ? CALC_LAYOUT.COLORS.loss
          : CALC_LAYOUT.COLORS.neutral;
    const sign = o.providerProfit >= 0 ? '+' : '';

    html += `<div class="calc-outcome">
      <strong>${CALC_LAYOUT.SCENARIO_LABELS[o.scenario] ?? o.scenario}</strong>
      <span>Requester outflow: ${formatMsat(o.requesterOutflow)}</span>
      <span>Provider inflow: ${formatMsat(o.providerInflow)}</span>
      <span style="color:${profitColor}">Provider profit: ${sign}${formatMsat(o.providerProfit)}</span>
    </div>`;
  }

  // Break-even threshold
  const thresholdText = Number.isFinite(threshold)
    ? `q* = ${threshold.toFixed(2)}`
    : 'never (always at loss)';
  html += `<div class="calc-summary">Break-even quality threshold: <strong>${thresholdText}</strong></div>`;

  // Loss warning (Req 21.5)
  if (atLoss) {
    html += `<div class="calc-warning" style="color:${CALC_LAYOUT.COLORS.warning}">⚠ Provider operates at a loss even at full quality (base + success &lt; cost).</div>`;
  }

  resultsEl.innerHTML = html;
}

/**
 * Initialize the settlement calculator widget.
 * Creates input controls for base fee, success fee, and cost,
 * and updates the display on every input change.
 */
export function initSettlementCalculator(container: HTMLElement): void {
  // Controls section
  const controlsEl = document.createElement('div');
  controlsEl.className = 'calc-controls';

  const baseFeeInput = createInputControl(
    controlsEl,
    'Base fee (msat)',
    'calc-base-fee',
    CALC_LAYOUT.DEFAULT_BASE_FEE,
  );
  const successFeeInput = createInputControl(
    controlsEl,
    'Success fee (msat)',
    'calc-success-fee',
    CALC_LAYOUT.DEFAULT_SUCCESS_FEE,
  );
  const costInput = createInputControl(
    controlsEl,
    'Provider cost (msat)',
    'calc-cost',
    CALC_LAYOUT.DEFAULT_COST,
  );

  container.appendChild(controlsEl);

  // Results section
  const resultsEl = document.createElement('div');
  resultsEl.className = 'calc-results';
  container.appendChild(resultsEl);

  function update(): void {
    const inputs: SettlementInputs = {
      baseFee: +baseFeeInput.value,
      successFee: +successFeeInput.value,
      cost: +costInput.value,
    };

    const outcomes = computeSettlementOutcomes(inputs);
    const threshold = computeBreakEvenThreshold(inputs);
    const atLoss = isProviderAtLoss(inputs);

    renderOutcomes(resultsEl, outcomes, atLoss, threshold);
  }

  baseFeeInput.addEventListener('input', update);
  successFeeInput.addEventListener('input', update);
  costInput.addEventListener('input', update);

  // Initial render
  update();
}
