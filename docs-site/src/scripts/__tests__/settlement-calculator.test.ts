import { describe, it, expect } from 'vitest';
import {
  computeSettlementOutcomes,
  computeBreakEvenThreshold,
  isProviderAtLoss,
} from '../settlement-calculator';

describe('computeSettlementOutcomes', () => {
  it('computes success outcome: requester outflow = baseFee + successFee, provider inflow = baseFee + successFee, profit = baseFee + successFee - cost (Req 21.2)', () => {
    const outcomes = computeSettlementOutcomes({ baseFee: 3000, successFee: 5000, cost: 4000 });
    const success = outcomes.find(o => o.scenario === 'success')!;
    expect(success.requesterOutflow).toBe(8000);  // 3000 + 5000
    expect(success.providerInflow).toBe(8000);     // 3000 + 5000
    expect(success.providerProfit).toBe(4000);     // 3000 + 5000 - 4000
  });

  it('computes failure outcome: requester outflow = baseFee, provider inflow = baseFee, profit = baseFee - cost (Req 21.2)', () => {
    const outcomes = computeSettlementOutcomes({ baseFee: 3000, successFee: 5000, cost: 4000 });
    const failure = outcomes.find(o => o.scenario === 'failure')!;
    expect(failure.requesterOutflow).toBe(3000);
    expect(failure.providerInflow).toBe(3000);
    expect(failure.providerProfit).toBe(-1000);    // 3000 - 4000
  });

  it('computes free outcome: zero transfer, profit = -cost (Req 21.2)', () => {
    const outcomes = computeSettlementOutcomes({ baseFee: 3000, successFee: 5000, cost: 4000 });
    const free = outcomes.find(o => o.scenario === 'free')!;
    expect(free.requesterOutflow).toBe(0);
    expect(free.providerInflow).toBe(0);
    expect(free.providerProfit).toBe(-4000);       // -cost
  });

  it('returns exactly three outcomes in order: success, failure, free', () => {
    const outcomes = computeSettlementOutcomes({ baseFee: 1000, successFee: 2000, cost: 500 });
    expect(outcomes).toHaveLength(3);
    expect(outcomes[0].scenario).toBe('success');
    expect(outcomes[1].scenario).toBe('failure');
    expect(outcomes[2].scenario).toBe('free');
  });

  it('provider profit at success = baseFee + successFee - cost (Req 21.3)', () => {
    const outcomes = computeSettlementOutcomes({ baseFee: 1000, successFee: 9000, cost: 7000 });
    expect(outcomes[0].providerProfit).toBe(3000); // 1000 + 9000 - 7000
  });

  it('provider profit at failure = baseFee - cost (Req 21.3)', () => {
    const outcomes = computeSettlementOutcomes({ baseFee: 1000, successFee: 9000, cost: 7000 });
    expect(outcomes[1].providerProfit).toBe(-6000); // 1000 - 7000
  });
});

describe('computeBreakEvenThreshold', () => {
  it('returns max(0, (cost - baseFee) / successFee) clamped to [0,1] when successFee > 0 (Req 21.4)', () => {
    // (4000 - 3000) / 5000 = 0.2
    expect(computeBreakEvenThreshold({ baseFee: 3000, successFee: 5000, cost: 4000 })).toBe(0.2);
    // (8000 - 2000) / 10000 = 0.6
    expect(computeBreakEvenThreshold({ baseFee: 2000, successFee: 10000, cost: 8000 })).toBe(0.6);
  });

  it('clamps to 0 when baseFee >= cost', () => {
    // baseFee covers cost entirely → always profitable
    expect(computeBreakEvenThreshold({ baseFee: 5000, successFee: 3000, cost: 2000 })).toBe(0);
    expect(computeBreakEvenThreshold({ baseFee: 5000, successFee: 3000, cost: 5000 })).toBe(0);
  });

  it('clamps to 1 when (cost - baseFee) / successFee > 1', () => {
    // (10000 - 1000) / 2000 = 4.5 → clamped to 1
    expect(computeBreakEvenThreshold({ baseFee: 1000, successFee: 2000, cost: 10000 })).toBe(1);
  });

  it('returns Infinity when successFee = 0 and baseFee < cost', () => {
    expect(computeBreakEvenThreshold({ baseFee: 1000, successFee: 0, cost: 5000 })).toBe(Infinity);
    expect(computeBreakEvenThreshold({ baseFee: 0, successFee: 0, cost: 1 })).toBe(Infinity);
  });

  it('returns 0 when successFee = 0 and baseFee >= cost', () => {
    expect(computeBreakEvenThreshold({ baseFee: 5000, successFee: 0, cost: 3000 })).toBe(0);
    expect(computeBreakEvenThreshold({ baseFee: 5000, successFee: 0, cost: 5000 })).toBe(0);
  });
});

describe('isProviderAtLoss', () => {
  it('returns true when baseFee + successFee < cost (Req 21.5)', () => {
    expect(isProviderAtLoss({ baseFee: 1000, successFee: 2000, cost: 5000 })).toBe(true);
    expect(isProviderAtLoss({ baseFee: 0, successFee: 0, cost: 1 })).toBe(true);
  });

  it('returns false when baseFee + successFee >= cost (Req 21.5)', () => {
    expect(isProviderAtLoss({ baseFee: 3000, successFee: 5000, cost: 4000 })).toBe(false);
    // Exactly equal → not at loss
    expect(isProviderAtLoss({ baseFee: 2000, successFee: 3000, cost: 5000 })).toBe(false);
  });
});
