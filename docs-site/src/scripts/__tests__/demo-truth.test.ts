import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { STEPS } from '../demo/steps';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '../../../..');

function readRepoFile(path: string): string {
  return readFileSync(resolve(repoRoot, path), 'utf8');
}

describe('interactive demo truth boundaries', () => {
  const steps = readRepoFile('docs-site/src/scripts/demo/steps.ts');

  it('keeps signature and identity examples aligned with the kernel', () => {
    expect(steps).toContain('BIP340 Schnorr');
    expect(steps).toContain('&lt;64-byte BIP340 Schnorr signature hex&gt;');
    expect(steps).toContain('&lt;64-hex secp256k1 x-only public key&gt;');
    expect(steps).toContain('&lt;same 64-hex public key&gt;');
    expect(steps).not.toContain('3045022100');
    expect(steps).not.toContain('public_key":"02');
  });

  it('does not present stake-backed reputation as live behavior', () => {
    expect(steps).toContain('Stake-backed identity is roadmap, not a live guarantee');
    expect(steps).toContain('not live: stake-backed slashing');
    expect(steps).toContain('stake         roadmap trust signal');
    expect(steps).not.toContain('marketplace.stake');
    expect(steps).not.toContain('total_staked_msat');
    expect(steps).not.toContain('register + stake');
    expect(steps).not.toContain('Stake costs real sats');
    expect(steps).not.toContain('Cheating burns the stake');
    expect(steps).not.toContain('Honesty is the Nash equilibrium');
    expect(steps).not.toContain('always irrational');
  });

  it('keeps receipt and integration claims explicit about their limits', () => {
    expect(steps).toContain('does not prove result correctness by itself');
    expect(steps).toContain('not automatic correctness');
    expect(steps).toContain('local and hosted API surfaces still use normal auth tokens');
    expect(steps).toContain('Authorization: Bearer &lt;runtime-token&gt;');
    expect(steps).not.toContain('No API keys. No platforms. Just signed deals.');
    expect(steps).not.toContain('cryptography and staked reputation');
    expect(steps).not.toContain('verify execution?');
  });

  it('labels every terminal block that is not complete raw stdout', () => {
    expect(steps).toContain('FROGLET_PUBLISH_DEMO_SERVICES=1');
    expect(steps).toContain('STARTUP OUTPUT (ABRIDGED)');
    expect(steps).toContain('IDENTITY OUTPUT (ABRIDGED)');
    expect(steps).toContain('SIGNED ENVELOPE SHAPE (ABRIDGED)');
    expect(steps).toContain('SERVICES OUTPUT (ABRIDGED)');
    expect(steps).toContain('RUNTIME DEAL REQUEST (ABRIDGED)');
    expect(steps).toContain('DEAL OUTPUT (ABRIDGED)');
    expect(steps).toContain('SETTLEMENT TERMS (EXAMPLE)');
    expect(steps).toContain('TRUST MODEL SUMMARY');
    expect(steps).toContain('CAPABILITIES OUTPUT (ABRIDGED)');
    expect(steps).toContain('INTEGRATION SUMMARY (NOT CLI STDOUT)');
    expect(steps).toContain('WHAT IS PROVEN TODAY (SUMMARY)');
    expect(steps).not.toContain('froglet-node</span> listening on 127.0.0.1:8080');
    expect(steps).not.toContain('-d @demo.add.json');
  });

  it('keeps the settlement step in sequence-diagram form', () => {
    const settlement = STEPS.find((step) => step.t === 'Settlement');
    expect(settlement).toBeDefined();

    expect(settlement?.board.nodes).toEqual([
      expect.objectContaining({ id: 'fr', lifeline: expect.any(Number), y: 0.12 }),
      expect.objectContaining({ id: 'fp', lifeline: expect.any(Number), y: 0.12 }),
    ]);

    expect(settlement?.board.arrows).toEqual([
      expect.objectContaining({ from: 'fr', to: 'fp', label: '1. accept: base fee locks', y: 0.30 }),
      expect.objectContaining({ from: 'fr', to: 'fp', label: '2. success hold accepted', y: 0.43 }),
      expect.objectContaining({ from: 'fp', to: 'fr', label: '3. receipt settles success', y: 0.58 }),
    ]);
  });
});
