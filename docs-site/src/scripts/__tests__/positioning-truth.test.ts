import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '../../../..');

function readRepoFile(path: string): string {
  return readFileSync(resolve(repoRoot, path), 'utf8');
}

/**
 * `demo-truth.test.ts` bans overclaims in the interactive demo copy. The same
 * bans belong on the top-of-funnel surfaces, which reach far more readers and
 * were previously unguarded — the homepage stated an unimplemented staking
 * mechanism in the present tense while the demo copy was forbidden from doing
 * so (DISCOVERY.md finding F3).
 */
const TOP_OF_FUNNEL = [
  'docs-site/src/pages/index.astro',
  'README.md',
] as const;

/**
 * Claims that describe mechanisms which do not exist. Each is either
 * contradicted by a "designed, not live" note elsewhere in the docs, or asserts
 * a guarantee cryptography cannot provide.
 */
const BANNED_CLAIMS = [
  'Cheating burns the stake',
  'Stake slashed',
  'Honesty is the Nash equilibrium',
  'Stake costs real sats',
  'reputation zeroed',
  'delivered as agreed',
  'guarantees the work was correct',
  'proves the result is correct',
] as const;

describe('top-of-funnel positioning truth', () => {
  for (const path of TOP_OF_FUNNEL) {
    describe(path, () => {
      const source = readRepoFile(path);

      it('does not state unimplemented mechanisms as live behavior', () => {
        const found = BANNED_CLAIMS.filter((claim) =>
          source.toLowerCase().includes(claim.toLowerCase()),
        );
        expect(found, `${path} makes claims the code does not support`).toEqual([]);
      });

      it('does not describe an attested fact as cryptographically proven', () => {
        // docs/SPEC.md SPEC-SET-1. Stripe settlement is attested, never proven.
        const overreach = /crypto\w*\s+(?:proof|proven|guarantee\w*)[^.]{0,60}stripe/i;
        expect(overreach.test(source), `${path} overstates Stripe's evidence`).toBe(false);
      });
    });
  }

  it('keeps the artifact-count claim consistent with the kernel', () => {
    // DISCOVERY.md F7: comparison.mdx said "six-artifact chain" then listed
    // five. The kernel defines six chain artifacts including InvoiceBundle.
    const comparison = readRepoFile('docs-site/src/content/docs/learn/comparison.mdx');
    const sixArtifact = comparison.match(/\*\*Signed (\w+)-artifact chain\*\*[^|]*/);
    if (sixArtifact) {
      const listed = ['descriptor', 'offer', 'quote', 'deal', 'receipt', 'invoice'].filter((name) =>
        sixArtifact[0].toLowerCase().includes(name),
      );
      const claimedCount = sixArtifact[1];
      const expected: Record<string, number> = { five: 5, six: 6 };
      expect(
        listed.length,
        `comparison.mdx claims a ${claimedCount}-artifact chain but lists ${listed.length}: ${listed.join(', ')}`,
      ).toBe(expected[claimedCount] ?? listed.length);
    }
  });

  it('renders payment-rail statuses from the maturity data, not hardcoded copy', () => {
    // Hand-maintained status labels are how the homepage ended up claiming
    // "Standardized" and "Paid staging" with nothing behind the words. The
    // rail rows must come from src/data/maturity.ts, whose evidence paths are
    // existence-checked in maturity-data.test.ts.
    const home = readRepoFile('docs-site/src/pages/index.astro');
    expect(home).toMatch(/import \{[^}]*RAILS[^}]*\} from '\.\.\/data\/maturity'/);
    expect(home).toContain('RAILS.map');
    for (const stale of ['>Standardized<', '>Paid staging<', '>Local<']) {
      expect(home, `stale hardcoded rail status ${stale}`).not.toContain(stale);
    }
  });

  it('does not claim browser-side cryptographic verification until the page does it', () => {
    // WASM behavior is covered by receipt-verifier.test.ts. This copy guard
    // prevents the page from both claiming and disclaiming signature checks.
    const page = readRepoFile('docs-site/src/pages/verify-receipt.astro');
    const disclaims = /does not verify[^.]*signature/i.test(page);
    const advertises = /verifies[^.]{0,40}(BIP.?340|schnorr signature)/i.test(page);
    expect(
      disclaims && advertises,
      'verify-receipt.astro both disclaims and advertises signature verification',
    ).toBe(false);
  });
});
