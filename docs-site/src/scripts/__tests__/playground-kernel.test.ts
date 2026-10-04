import { describe, expect, it } from 'vitest';
import { kernel, sorted, vectors, verifier } from './playground-helpers';

// The playground's keys, signatures, hashes and verdicts come from Rust compiled to WebAssembly. These tests hold the
// TypeScript adapters over it to the frozen conformance vectors, so a wrong adapter cannot pass by agreeing with itself.

const seedFor = (signer: string) => (signer === vectors.keys.provider_id ? vectors.keys.provider_seed_hex : vectors.keys.requester_seed_hex);
const signable = ['descriptor', 'offer', 'quote', 'deal', 'receipt'];

describe('kernel adapter', () => {
  it('signs every vector artifact byte for byte, with the vectors\' own seeds', () => {
    let signed = 0;
    for (const [name, entry] of Object.entries<any>(vectors.artifacts)) {
      const artifact = entry.artifact;
      if (!signable.includes(artifact.artifact_type)) continue;
      const produced = kernel.sign(seedFor(artifact.signer), artifact.artifact_type, artifact.created_at, artifact.payload);
      expect(produced, name).toEqual(artifact);
      signed += 1;
    }
    expect(signed).toBe(9);
  });

  it('hashes payloads the way the vectors record them, whatever the key order', () => {
    for (const [name, entry] of Object.entries<any>(vectors.artifacts)) {
      expect(kernel.hashJson(entry.artifact.payload), name).toBe(entry.payload_hash);
      expect(kernel.hashJson(sorted(entry.artifact.payload)), `${name} with sorted keys`).toBe(entry.payload_hash);
    }
  });

  it('hashes the vector workload, its input and its module', () => {
    const { submission } = vectors.workload_spec;
    expect(kernel.hashJson(submission.input)).toBe(submission.workload.input_hash);
    expect(kernel.hashJson(submission.workload)).toBe(vectors.artifacts.free_quote.artifact.payload.workload_hash);
    const module = Uint8Array.from(submission.module_bytes_hex.match(/../g).map((pair: string) => parseInt(pair, 16)));
    expect(kernel.hashBytes(module)).toBe(submission.workload.module_hash);
  });

  it('makes identities that are new each time and whose key follows from the seed', () => {
    const first = kernel.newIdentity();
    const second = kernel.newIdentity();
    expect(first.seed_hex).toMatch(/^[0-9a-f]{64}$/);
    expect(first.public_key).toMatch(/^[0-9a-f]{64}$/);
    expect(first.seed_hex).not.toBe(second.seed_hex);
    expect(kernel.publicKey(first.seed_hex)).toBe(first.public_key);
    expect(kernel.publicKey(vectors.keys.provider_seed_hex)).toBe(vectors.keys.provider_id);
  });

  it('says why text is not JSON in the words the Rust parser uses, and says nothing about JSON', () => {
    expect(kernel.jsonError('{"a": 1}')).toBeNull();
    expect(kernel.jsonError('  [1, 2, 3]  ')).toBeNull();
    expect(kernel.jsonError('hello')).toBe('expected value at line 1 column 1');
    expect(kernel.jsonError('')).toBe('EOF while parsing a value at line 1 column 0');
    expect(kernel.jsonError('{"unfinished": ')).toMatch(/^EOF while parsing a value at line 1 column \d+$/);
    expect(kernel.jsonError('{"a": 1} extra')).toBe('trailing characters at line 1 column 10');
    // The node's parser is stricter than a browser's in places, and the kernel's is the node's.
    for (const text of ['"\\ud800"', '1e999']) {
      expect(() => JSON.parse(text), `a browser accepts ${text}`).not.toThrow();
      expect(kernel.jsonError(text), `the kernel does not accept ${text}`).not.toBeNull();
    }
  });

  it('refuses what it should not sign', () => {
    const seed = vectors.keys.provider_seed_hex;
    expect(() => kernel.sign(seed, 'invoice_bundle' as any, 1, {})).toThrow();
    expect(() => kernel.sign('zz', 'offer', 1, {})).toThrow();
  });
});

describe('verifier adapter', () => {
  it('verifies every vector artifact and the free chain the vectors define', () => {
    for (const [name, entry] of Object.entries<any>(vectors.artifacts)) {
      const report = verifier.verifyDocument(entry.artifact);
      expect(report.envelope_valid, name).toBe(true);
    }
    const order: string[] = vectors.free_service_conformance_path.artifact_order;
    const chain = verifier.validateChain(order.map((name) => vectors.artifacts[name].artifact));
    expect(chain.valid).toBe(true);
    expect(chain.artifacts.map((artifact) => artifact.status)).toEqual(order.map(() => 'verified'));
  });

  it('rejects a change after signing and a signature that is not the signer\'s', () => {
    const receipt = structuredClone(vectors.artifacts.free_receipt.artifact);
    receipt.payload.result_hash = '0'.repeat(64);
    expect(verifier.verifyDocument(receipt).status).toBe('invalid');

    const genuine = vectors.artifacts.free_deal.artifact;
    const wrongKey = kernel.sign(vectors.keys.provider_seed_hex, 'deal', genuine.created_at, genuine.payload);
    const report = verifier.verifyDocument({ ...wrongKey, signer: vectors.keys.requester_id });
    expect(report.status).toBe('invalid');
  });

  it('does not call a chain valid when a link is missing', () => {
    const order: string[] = vectors.free_service_conformance_path.artifact_order;
    const withoutQuote = order.filter((name) => name !== 'free_quote').map((name) => vectors.artifacts[name].artifact);
    expect(verifier.validateChain(withoutQuote).valid).toBe(false);
  });
});
