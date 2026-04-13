import { describe, it, expect } from 'vitest';
import { computeCheatPayoff, computeRatio } from '../trust-graph';

describe('computeCheatPayoff', () => {
  it('returns fee - stake (Req 17.1)', () => {
    expect(computeCheatPayoff(500, 100)).toBe(-400);
    expect(computeCheatPayoff(100, 100)).toBe(0);
    expect(computeCheatPayoff(50, 100)).toBe(50);
    expect(computeCheatPayoff(0, 100)).toBe(100);
    expect(computeCheatPayoff(200, 300)).toBe(100);
  });
});

describe('computeRatio', () => {
  it('returns stake / fee (Req 17.2)', () => {
    expect(computeRatio(500, 100)).toBe(5);
    expect(computeRatio(100, 100)).toBe(1);
    expect(computeRatio(50, 100)).toBe(0.5);
    expect(computeRatio(0, 100)).toBe(0);
    expect(computeRatio(200, 300)).toBeCloseTo(2 / 3);
  });
});

describe('ratio and payoff relationship', () => {
  it('when ratio = 1.0 (stake = fee), payoff is exactly 0 (Req 17.3)', () => {
    const fee = 100;
    const stake = fee; // ratio = 1.0
    expect(computeRatio(stake, fee)).toBe(1.0);
    expect(computeCheatPayoff(stake, fee)).toBe(0);
  });

  it('when ratio > 1.0 (stake > fee), payoff is negative (Req 17.4)', () => {
    expect(computeCheatPayoff(500, 100)).toBeLessThan(0);
    expect(computeCheatPayoff(150, 100)).toBeLessThan(0);
    expect(computeCheatPayoff(1000, 999)).toBeLessThan(0);
  });

  it('when ratio < 1.0 (stake < fee), payoff is positive (Req 17.5)', () => {
    expect(computeCheatPayoff(50, 100)).toBeGreaterThan(0);
    expect(computeCheatPayoff(10, 100)).toBeGreaterThan(0);
    expect(computeCheatPayoff(0, 100)).toBeGreaterThan(0);
  });

  it('payoff follows linear function: payoff(r) = fee × (1 - r) (Req 17.6)', () => {
    const cases = [
      { stake: 500, fee: 100 },
      { stake: 100, fee: 100 },
      { stake: 50, fee: 100 },
      { stake: 0, fee: 100 },
      { stake: 200, fee: 300 },
      { stake: 750, fee: 500 },
    ];

    for (const { stake, fee } of cases) {
      const r = computeRatio(stake, fee);
      const expected = fee * (1 - r);
      expect(computeCheatPayoff(stake, fee)).toBeCloseTo(expected);
    }
  });
});
