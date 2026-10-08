// @vitest-environment node
// Tracked conformance keys and the actual frozen Rust kernel/verifier; transport
// is local. No C7 credential, publication, request or native job is involved.
import { expect, test } from 'vitest';
import { prepareLiveRun } from '../live-service-client';
import { PUBLIC_DEMO } from '../../data/public-demo-config';
import { kernel, verifier, vectors } from './playground-helpers';

const seed = '11'.repeat(32), provider = kernel.publicKey(seed);
const historical = kernel.sign(seed, 'descriptor', 1700000000, { ...vectors.artifacts.descriptor.artifact.payload, capabilities: { service_kinds: [PUBLIC_DEMO.catalogService, 'compute.wasm.v1'], execution_runtimes: ['builtin', 'wasm'] } });
const now = Math.floor(Date.now() / 1000);

function fixture(alter: (record: any) => void = () => {}, referenced = historical) {
  const fresh = kernel.sign(seed, 'descriptor', now, { ...historical.payload, descriptor_seq: 2 });
  const offer = kernel.sign(seed, 'offer', now, {
    ...vectors.artifacts.free_offer.artifact.payload,
    descriptor_hash: referenced.hash, offer_id: PUBLIC_DEMO.catalogService, offer_kind: PUBLIC_DEMO.catalogService,
    execution_profile: { runtime: 'builtin', package_kind: 'builtin', contract_version: 'froglet.builtin.data_query.json.v1', abi_version: 'froglet.builtin.data_query.json.v1', max_input_bytes: 1048576, max_output_bytes: 1048576, max_runtime_ms: 2000, max_memory_bytes: 0, fuel_limit: 0 },
  });
  expect(verifier.verifyDocument(fresh).status).toBe('verified');
  expect(verifier.verifyDocument(offer)).toEqual(expect.objectContaining({status: 'verified'}));
  const record = { hash: referenced.hash, kind: 'descriptor', actor_id: provider, cursor: 1, document: structuredClone(referenced) };
  alter(record);
  const seen: any[] = [];
  const deps = { kernel, verifier, providerId: provider, transport: async (request: any) => {
    seen.push(structuredClone(request));
    if (request.path === '/v1/provider/descriptor') return { status: 200, body: fresh };
    if (request.path === '/v1/provider/offers') return { status: 200, body: { offers: [offer] } };
    if (request.path === '/v1/artifacts/' + referenced.hash) return { status: 200, body: record };
    if (request.path === '/v1/provider/services/' + PUBLIC_DEMO.catalogService) return { status: 200, body: { service: { provider_id: provider, offer_id: PUBLIC_DEMO.catalogService, service_id: PUBLIC_DEMO.catalogService, runtime: 'builtin', package_kind: 'builtin', entrypoint_kind: 'builtin', entrypoint: PUBLIC_DEMO.catalogService, contract_version: 'froglet.builtin.data_query.json.v1', binding_hash: 'a'.repeat(64), settlement_method: 'none' } } };
    if (request.path === '/v1/provider/quotes') return { status: 201, body: kernel.sign(seed, 'quote', now, { provider_id: provider, requester_id: request.body.requester_id, descriptor_hash: referenced.hash, offer_hash: offer.hash, workload_kind: PUBLIC_DEMO.catalogService, workload_hash: kernel.hashJson(request.body.execution), expires_at: now + 30, settlement_terms: { method: 'none', destination_identity: '', base_fee_msat: 0, success_fee_msat: 0, max_base_invoice_expiry_secs: 0, max_success_hold_expiry_secs: 0, min_final_cltv_expiry: 0 }, execution_limits: { max_input_bytes: 1048576, max_output_bytes: 1048576, max_runtime_ms: 2000, max_memory_bytes: 0, fuel_limit: 0 } }) };
    throw new Error('Unexpected fixture transport: ' + request.path);
  } };
  return { deps, seen, offer, fresh, record };
}

test('a valid unchanged offer uses its exact signed historical descriptor when current metadata advances', async () => {
  const f = fixture(), before = kernel.canonicalize(f.offer);
  expect(f.fresh.hash).not.toBe(historical.hash);
  const run = await prepareLiveRun(f.deps, '{"op":"describe"}');
  expect(run.artifacts.descriptor).toEqual(historical);
  expect(run.artifacts.quote.payload.descriptor_hash).toBe(historical.hash);
  expect(kernel.canonicalize(run.artifacts.offer)).toBe(before);
  expect(f.seen.filter(r => r.path.startsWith('/v1/artifacts/')).map(r => r.path)).toEqual(['/v1/artifacts/' + historical.hash]);
  expect(verifier.validateChain([run.artifacts.descriptor, run.artifacts.offer, run.artifacts.quote, run.artifacts.deal]).valid).toBe(true);
});

test.each(['record-hash', 'record-actor', 'record-kind', 'document-hash', 'signature', 'payload'])('altered referenced descriptor %s fails before requesting a quote', async kind => {
  const f = fixture(record => {
    if (kind === 'record-hash') record.hash = 'a'.repeat(64);
    if (kind === 'record-actor') record.actor_id = 'a'.repeat(64);
    if (kind === 'record-kind') record.kind = 'offer';
    if (kind === 'document-hash') record.document.hash = 'a'.repeat(64);
    if (kind === 'signature') record.document.signature = '0'.repeat(128);
    if (kind === 'payload') record.document.payload.descriptor_seq += 1;
  });
  await expect(prepareLiveRun(f.deps, '{"op":"describe"}')).rejects.toThrow();
  expect(f.seen.some(r => r.path === '/v1/provider/quotes')).toBe(false);
});

test('a genuinely signed but expired historical descriptor cannot authorize new work', async () => {
  const expired = kernel.sign(seed, 'descriptor', now - 2, { ...historical.payload, expires_at: now - 1 });
  expect(verifier.verifyDocument(expired).status).toBe('verified'); // No clock is supplied to this offline verdict.
  const f = fixture(() => {}, expired);
  await expect(prepareLiveRun(f.deps, '{"op":"describe"}')).rejects.toThrow();
  expect(f.seen.some(r => r.path === '/v1/provider/quotes')).toBe(false);
});

test('missing referenced artifacts stop discovery rather than rebinding the signed offer', async () => {
  const f = fixture(), transport = f.deps.transport;
  f.deps.transport = async request => request.path.startsWith('/v1/artifacts/') ? { status: 404, body: { error: 'Selected artifact unavailable' } } : transport(request);
  await expect(prepareLiveRun(f.deps, '{"op":"describe"}')).rejects.toThrow();
  expect(f.seen.some(r => r.path === '/v1/provider/quotes')).toBe(false);
});
