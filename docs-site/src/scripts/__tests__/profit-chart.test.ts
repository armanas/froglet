import { describe, it, expect } from 'vitest';
import {
  computeProviderPayoff,
  computeRequesterPayoff,
  computeBreakEven,
} from '../profit-chart';

describe('computeProviderPayoff', () => {
  it('returns baseFee + q * successFee - cost (Req 18.4)', () => {
    // Economics page defaults: baseFee=3, successFee=5, cost=2, value=10
    expect(computeProviderPayoff(3, 5, 2, 0)).toBe(1);     // 3 + 0*5 - 2 = 1
    expect(computeProviderPayoff(3, 5, 2, 0.5)).toBe(3.5);  // 3 + 0.5*5 - 2 = 3.5
    expect(computeProviderPayoff(3, 5, 2, 1)).toBe(6);      // 3 + 1*5 - 2 = 6
  });

  it('handles zero quality (q=0)', () => {
    expect(computeProviderPayoff(10, 20, 5, 0)).toBe(5);    // 10 + 0 - 5
    expect(computeProviderPayoff(2, 10, 8, 0)).toBe(-6);    // 2 + 0 - 8
  });

  it('handles full quality (q=1)', () => {
    expect(computeProviderPayoff(10, 20, 5, 1)).toBe(25);   // 10 + 20 - 5
    expect(computeProviderPayoff(0, 0, 10, 1)).toBe(-10);   // 0 + 0 - 10
  });

  it('handles zero fees and cost', () => {
    expect(computeProviderPayoff(0, 0, 0, 0.5)).toBe(0);
  });
});

describe('computeRequesterPayoff', () => {
  it('returns q * value - baseFee - q * successFee (Req 18.5)', () => {
    // Economics page defaults: baseFee=3, successFee=5, cost=2, value=10
    expect(computeRequesterPayoff(10, 3, 5, 0)).toBe(-3);    // 0*10 - 3 - 0*5 = -3
    expect(computeRequesterPayoff(10, 3, 5, 0.5)).toBe(-0.5); // 0.5*10 - 3 - 0.5*5 = -0.5
    expect(computeRequesterPayoff(10, 3, 5, 1)).toBe(2);      // 1*10 - 3 - 1*5 = 2
  });

  it('handles zero quality (q=0)', () => {
    expect(computeRequesterPayoff(100, 10, 20, 0)).toBe(-10); // 0 - 10 - 0
  });

  it('handles full quality (q=1)', () => {
    expect(computeRequesterPayoff(100, 10, 20, 1)).toBe(70);  // 100 - 10 - 20
  });

  it('handles zero value', () => {
    expect(computeRequesterPayoff(0, 5, 3, 1)).toBe(-8);      // 0 - 5 - 3
  });
});

describe('computeBreakEven', () => {
  it('returns max(0, (cost - baseFee) / successFee) when successFee > 0 (Req 18.6)', () => {
    // Economics page defaults: baseFee=3, successFee=5, cost=2
    // (2 - 3) / 5 = -0.2 → max(0, -0.2) = 0
    expect(computeBreakEven(3, 5, 2)).toBe(0);

    // cost > baseFee: (8 - 2) / 10 = 0.6
    expect(computeBreakEven(2, 10, 8)).toBe(0.6);

    // cost = baseFee: (5 - 5) / 10 = 0 → max(0, 0) = 0
    expect(computeBreakEven(5, 10, 5)).toBe(0);
  });

  it('returns Infinity when successFee = 0 and baseFee < cost (Req 18.6)', () => {
    expect(computeBreakEven(2, 0, 10)).toBe(Infinity);
    expect(computeBreakEven(0, 0, 1)).toBe(Infinity);
  });

  it('returns 0 when baseFee >= cost (successFee = 0) (Req 18.6)', () => {
    expect(computeBreakEven(10, 0, 5)).toBe(0);
    expect(computeBreakEven(5, 0, 5)).toBe(0);
  });
});
