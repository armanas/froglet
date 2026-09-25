import { describe, expect, it } from 'vitest';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { MATURITY, MATURITY_LADDER, RAILS, type MaturityStatus } from '../../data/maturity';

const repoRoot = resolve(__dirname, '../../../..');

/**
 * The anti-drift contract. Maturity labels on the site are only trustworthy if
 * every one of them points at something a reader can open, so these tests fail
 * the build when evidence disappears rather than letting a stale badge ship.
 */
describe('maturity data', () => {
  it('has at least one entry and unique ids', () => {
    expect(MATURITY.length).toBeGreaterThan(0);
    const ids = MATURITY.map((entry) => entry.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it('points every status at evidence that exists in the repo', () => {
    const missing = MATURITY.filter((entry) => !existsSync(resolve(repoRoot, entry.evidence))).map(
      (entry) => `${entry.id} -> ${entry.evidence}`,
    );
    expect(missing, 'maturity entries whose evidence file no longer exists').toEqual([]);
  });

  it('uses only statuses from the published ladder', () => {
    const rungs = Object.keys(MATURITY_LADDER) as MaturityStatus[];
    for (const entry of MATURITY) {
      expect(rungs, `${entry.id} has an off-ladder status`).toContain(entry.status);
    }
  });

  it('requires an ISO verified date on every entry', () => {
    for (const entry of MATURITY) {
      expect(entry.verified, `${entry.id} verified date`).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    }
  });

  it('writes notes that state what ran, not what is intended', () => {
    // Aspirational language is how honest maturity tables rot. Catch the
    // common offenders at the point where the copy is authored.
    const aspirational = /\b(will|soon|planned|coming soon|intended to|aims to)\b/i;
    for (const entry of MATURITY) {
      expect(aspirational.test(entry.note), `${entry.id} note is aspirational: "${entry.note}"`).toBe(
        false,
      );
    }
  });

  it('keeps no payment rail above prototype without a live transcript', () => {
    // The launch-posture rule from docs/PAYMENT_MATRIX.md: no rail may claim
    // production until a public payment transcript exists. None does today.
    for (const rail of RAILS) {
      expect(rail.status, `${rail.id} claims production without a committed transcript`).not.toBe(
        'production',
      );
    }
  });

  it('does not claim x402 is live before its publish path and transcript land', () => {
    const x402 = MATURITY.find((entry) => entry.id === 'x402');
    expect(x402).toBeDefined();
    expect(x402!.status).toBe('prototype');
    expect(x402!.note).toMatch(/no live transcript/i);
  });

  it('records the verifier and conformance vectors as the evidence-layer proof', () => {
    const verifier = MATURITY.find((entry) => entry.id === 'verifier');
    expect(verifier?.evidence).toBe('froglet-verify/tests/conformance.rs');
    const conformance = MATURITY.find((entry) => entry.id === 'conformance');
    expect(conformance?.evidence).toBe('conformance/kernel_v1.json');
  });
});
