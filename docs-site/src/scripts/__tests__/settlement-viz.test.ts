import { describe, it, expect } from 'vitest';
import {
  computeRequesterOutflow,
  computeProviderInflow,
  formatMsat,
  buildScenarios,
} from '../settlement-viz';

describe('computeRequesterOutflow', () => {
  it('returns B + F for success scenario (Req 18.1)', () => {
    expect(computeRequesterOutflow(3000, 5000, 'success')).toBe(8000);
    expect(computeRequesterOutflow(1000, 4000, 'success')).toBe(5000);
  });

  it('returns B for failure scenario (Req 18.2)', () => {
    expect(computeRequesterOutflow(3000, 5000, 'failure')).toBe(3000);
    expect(computeRequesterOutflow(1000, 4000, 'failure')).toBe(1000);
  });

  it('returns 0 for free scenario (Req 18.3)', () => {
    expect(computeRequesterOutflow(3000, 5000, 'free')).toBe(0);
    expect(computeRequesterOutflow(0, 0, 'free')).toBe(0);
  });
});

describe('computeProviderInflow', () => {
  it('returns B + F for success scenario (Req 18.1)', () => {
    expect(computeProviderInflow(3000, 5000, 'success')).toBe(8000);
    expect(computeProviderInflow(1000, 4000, 'success')).toBe(5000);
  });

  it('returns B for failure scenario (Req 18.2)', () => {
    expect(computeProviderInflow(3000, 5000, 'failure')).toBe(3000);
    expect(computeProviderInflow(1000, 4000, 'failure')).toBe(1000);
  });

  it('returns 0 for free scenario (Req 18.3)', () => {
    expect(computeProviderInflow(3000, 5000, 'free')).toBe(0);
    expect(computeProviderInflow(0, 0, 'free')).toBe(0);
  });
});

describe('formatMsat', () => {
  it('formats zero as "0"', () => {
    expect(formatMsat(0)).toBe('0');
  });

  it('formats values with comma separators and msat suffix', () => {
    expect(formatMsat(8000)).toBe('8,000 msat');
    expect(formatMsat(3000)).toBe('3,000 msat');
    expect(formatMsat(100000)).toBe('100,000 msat');
  });
});

describe('buildScenarios', () => {
  const scenarios = buildScenarios(3000, 5000);

  it('success scenario: requester outflow = B + F, provider inflow = B + F (Req 18.1)', () => {
    expect(scenarios.ok.requesterTotal).toBe('8,000 msat');
    expect(scenarios.ok.providerTotal).toBe('8,000 msat');
    expect(scenarios.ok.base.state).toBe('settled');
    expect(scenarios.ok.success.state).toBe('settled');
  });

  it('failure scenario: requester outflow = B, provider inflow = B, success fee canceled (Req 18.2)', () => {
    expect(scenarios.fail.requesterTotal).toBe('3,000 msat');
    expect(scenarios.fail.providerTotal).toBe('3,000 msat');
    expect(scenarios.fail.base.state).toBe('settled');
    expect(scenarios.fail.success.state).toBe('canceled');
  });

  it('free scenario: both zero (Req 18.3)', () => {
    expect(scenarios.free.requesterTotal).toBe('0');
    expect(scenarios.free.providerTotal).toBe('0');
    expect(scenarios.free.base.state).toBe('n/a');
    expect(scenarios.free.success.state).toBe('n/a');
  });

  it('works with different fee values', () => {
    const custom = buildScenarios(1000, 4000);
    expect(custom.ok.requesterTotal).toBe('5,000 msat');
    expect(custom.ok.providerTotal).toBe('5,000 msat');
    expect(custom.fail.requesterTotal).toBe('1,000 msat');
    expect(custom.fail.providerTotal).toBe('1,000 msat');
    expect(custom.free.requesterTotal).toBe('0');
    expect(custom.free.providerTotal).toBe('0');
  });
});
