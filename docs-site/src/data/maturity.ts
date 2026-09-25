/**
 * Single source of truth for every maturity label shown on the site.
 *
 * The rule this file exists to enforce: a status shown to a reader must point
 * at something in the repo that substantiates it. `evidence` is a repo-relative
 * path to the test, fixture, transcript, or run log a skeptical reader can open;
 * `__tests__/maturity-data.test.ts` fails the build if any path stops existing,
 * so a deleted test cannot leave a dangling evidence link on the homepage.
 * Existence checks do not prove that a test passed or that a deployment works.
 * These are source maturity labels, not release or uptime certifications.
 *
 * Adding a row without evidence is not allowed. If something has no evidence
 * yet, its status is `spec` and its evidence is the document that specifies it.
 */

/**
 * The maturity ladder. Deliberately coarse — four rungs a reader can hold in
 * their head, with a hard boundary at `beta`: nothing reaches `beta` or above
 * without having run against something real.
 */
export const MATURITY_LADDER = {
  spec: {
    label: 'Spec',
    meaning: 'Documented and specified. No implementation to run yet.',
  },
  prototype: {
    label: 'Prototype',
    meaning: 'Runs locally. Incomplete error handling; not exercised against live endpoints.',
  },
  beta: {
    label: 'Beta',
    meaning: 'Works against live or hermetic endpoints. Known edge cases remain.',
  },
  production: {
    label: 'Production',
    meaning: 'Error handling, idempotency, and observability in place, with a public transcript.',
  },
} as const;

export type MaturityStatus = keyof typeof MATURITY_LADDER;

export interface MaturityEntry {
  /** Stable id, used as the React/Astro key and in tests. */
  id: string;
  name: string;
  /** What kind of thing this is: payment rail, runtime, agent client, … */
  kind: string;
  /** The one primitive this integration speaks. */
  primitive: string;
  status: MaturityStatus;
  /**
   * Repo-relative path to what substantiates the status. Checked for existence
   * by the test suite — this is the anti-drift mechanism.
   */
  evidence: string;
  /**
   * One sentence a reader can check. Must not overstate: say what ran, not what
   * is intended.
   */
  note: string;
  /** ISO date the evidence was last confirmed. */
  verified: string;
  logo?: string;
}

export const MATURITY: MaturityEntry[] = [
  {
    id: 'verifier',
    name: 'Offline verifier',
    kind: 'evidence',
    primitive: 'froglet-verify',
    status: 'beta',
    evidence: 'froglet-verify/tests/conformance.rs',
    note: 'Verifies a full chain offline with no node and no network; reproduces the frozen conformance vectors.',
    verified: '2026-07-31',
  },
  {
    id: 'conformance',
    name: 'Conformance vectors',
    kind: 'evidence',
    primitive: 'kernel_v1.json',
    status: 'beta',
    evidence: 'conformance/kernel_v1.json',
    note: 'Frozen byte-for-byte and re-verified by Rust runners sharing the protocol implementation and an independently implemented Python verifier.',
    verified: '2026-09-24',
  },
  {
    id: 'lightning',
    name: 'Lightning',
    kind: 'payment rail',
    primitive: 'bolt11 invoice',
    status: 'beta',
    evidence: 'docs/PAYMENT_MATRIX.md',
    note: 'Proven end-to-end on regtest with real LND. No mainnet payment has been made.',
    verified: '2026-05-15',
    logo: '/logos/lightning.svg',
  },
  {
    id: 'x402',
    name: 'x402',
    kind: 'payment rail',
    primitive: 'EIP-3009 transfer',
    status: 'prototype',
    evidence: 'conformance/x402_v1.json',
    note: 'Kernel settlement method and conformance vectors landed; not yet on the publish path and no live transcript on any network.',
    verified: '2026-07-31',
    logo: '/logos/x402.svg',
  },
  {
    id: 'stripe',
    name: 'Stripe',
    kind: 'payment rail',
    primitive: 'payment_intent',
    status: 'prototype',
    evidence: 'docs/PAYMENT_MATRIX.md',
    note: 'Test-mode Shared Payment Token flow against a mock and the Stripe sandbox. No live-money transcript.',
    verified: '2026-07-10',
    logo: '/logos/stripe.svg',
  },
  {
    id: 'claude-code',
    name: 'Claude Code',
    kind: 'agent client',
    primitive: 'MCP server',
    status: 'beta',
    evidence: 'integrations/mcp/froglet/test/server.test.mjs',
    note: 'Single `froglet` tool with the two-step publish contract, covered by the MCP server test suite.',
    verified: '2026-07-31',
    logo: '/logos/anthropic.svg',
  },
  {
    id: 'mcp',
    name: 'MCP',
    kind: 'transport',
    primitive: 'stdio',
    status: 'beta',
    evidence: 'integrations/mcp/froglet/test/server.test.mjs',
    note: 'Included as froglet-mcp in the source integrations; also available as a dependency-minimal native bridge.',
    verified: '2026-07-31',
    logo: '/logos/mcp.svg',
  },
  {
    id: 'openclaw',
    name: 'OpenClaw',
    kind: 'tool protocol',
    primitive: 'claw.json',
    status: 'beta',
    evidence: 'integrations/openclaw/froglet/README.md',
    note: 'Shares the single-tool dispatch library with the MCP surface.',
    verified: '2026-07-31',
    logo: '/logos/openclaw.svg',
  },
  {
    id: 'wasm',
    name: 'WASM',
    kind: 'runtime',
    primitive: 'wasmtime',
    status: 'beta',
    evidence: 'tests/runtime_routes.rs',
    note: 'Fuel-metered execution with declared limits; the most exercised runtime in the test suite.',
    verified: '2026-07-31',
    logo: '/logos/wasm.svg',
  },
  {
    id: 'docker',
    name: 'Docker',
    kind: 'runtime',
    primitive: 'OCI image',
    status: 'beta',
    evidence: 'docs/DOCKER.md',
    note: 'Digest-pinned OCI execution; images built and smoke-tested in CI.',
    verified: '2026-07-31',
    logo: '/logos/docker.svg',
  },
  {
    id: 'tor',
    name: 'Tor',
    kind: 'transport',
    primitive: 'onion v3',
    status: 'prototype',
    evidence: 'python/tests/test_tor_integration.py',
    note: 'Self-hosted onion registration path; exercised only behind an opt-in environment flag.',
    verified: '2026-07-31',
    logo: '/logos/tor.svg',
  },
  {
    id: 'dns-attestation',
    name: 'DNS attestation',
    kind: 'identity',
    primitive: 'TXT record',
    status: 'prototype',
    evidence: 'docs/IDENTITY_ATTESTATION.md',
    note: 'Subject-side signing and record formatting ship in the CLI; the issuing service is specified but not built.',
    verified: '2026-07-31',
  },
  {
    id: 'tee',
    name: 'TEE',
    kind: 'runtime',
    primitive: 'SGX · Nitro',
    status: 'spec',
    evidence: 'docs/CONFIDENTIAL.md',
    note: 'Specified as an additive extension. No implementation.',
    verified: '2026-07-31',
    logo: '/logos/lock.svg',
  },
  {
    id: 'selective-disclosure',
    name: 'Selective disclosure',
    kind: 'evidence',
    primitive: 'field commitments',
    status: 'spec',
    evidence: 'docs/SPEC.md',
    note: 'Deferred to a future minor version; v1 payload hashing commits to the whole payload.',
    verified: '2026-07-31',
  },
];

export const RAILS = MATURITY.filter((entry) => entry.kind === 'payment rail');

export function statusLabel(status: MaturityStatus): string {
  return MATURITY_LADDER[status].label;
}
