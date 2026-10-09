// @vitest-environment node
// Hermetic native-shaped HTTP-service snapshots. Transport/verdicts are fixtures;
// real deployed C7 signatures and execution require the separate operator gate.
import { expect, test, vi } from 'vitest';
import * as client from '../live-service-client';
import { PUBLIC_DEMO } from '../../data/public-demo-config';
import { kernel } from './playground-helpers';

const ids = client.PUBLIC_HTTP_SERVICE_IDS;
const profiles = ids.map((serviceId, index) => ({ serviceId, offerId: serviceId, offerHash: String(index + 1).repeat(64), bindingHash: String(index + 4).repeat(64), moduleHash: String(index + 4).repeat(64), revisionHash: String(index + 7).repeat(64), descriptorHash: 'a'.repeat(64), operationHash: ['b', 'c', 'd'][index].repeat(64), entrypoint: 'run' }));
const config = () => ({ providerId: PUBLIC_DEMO.providerId, publishedServices: { enabled: true, profiles: structuredClone(profiles) } });
const prepare = (...args: unknown[]) => (client as any).preparePublishedServiceRun(...args);

test('named tools require an explicit complete nine-field fixed C7 configuration', () => {
  expect(client.publishedServiceProfiles(config())).toEqual(profiles);
  for (const change of [null, {}, { ...config(), providerId: 'f'.repeat(64) }, { ...config(), publishedServices: { enabled: false, profiles } }, { ...config(), publishedServices: { enabled: true, profiles: profiles.slice(1) } }]) expect(client.publishedServiceProfiles(change)).toEqual([]);
  for (const field of ['offerHash', 'bindingHash', 'revisionHash', 'operationHash', 'moduleHash', 'descriptorHash']) {
    const bad = config(); (bad.publishedServices.profiles[0] as any)[field] = 'not a hash'; expect(client.publishedServiceProfiles(bad)).toEqual([]);
  }
  const bad = config(); (bad.publishedServices.profiles[0] as any).url = 'https://other.invalid'; expect(client.publishedServiceProfiles(bad)).toEqual([]);
  const authoringPath = config(); authoringPath.publishedServices.profiles[0].entrypoint = 'operation.wasm'; expect(client.publishedServiceProfiles(authoringPath)).toEqual([]);
});

function fixture(serviceId: typeof ids[number] = 'marketplace-provider', alter: (value: any, path: string) => void = () => {}) {
  const p = profiles.find(p => p.serviceId === serviceId)!;
  const cap = `net.http.operation.${p.operationHash}`;
  const now = Math.floor(Date.now() / 1000);
  const limits = { max_runtime_ms: 2000, max_memory_bytes: 8388608, fuel_limit: 50000000, max_input_bytes: 2048, max_output_bytes: 131072 };
  const descriptor = { artifact_type: 'descriptor', hash: p.descriptorHash, payload: { provider_id: PUBLIC_DEMO.providerId } };
  const offer = { artifact_type: 'offer', hash: p.offerHash, payload: { provider_id: PUBLIC_DEMO.providerId, offer_id: p.offerId, descriptor_hash: p.descriptorHash, offer_kind: 'compute.execution.v1', price_schedule: { base_fee_msat: 0, success_fee_msat: 0 }, settlement_method: 'none', execution_profile: { runtime: 'wasm', package_kind: 'inline_module', contract_version: 'froglet.wasm.host_json.v1', ...limits } } };
  const service = { service_id: serviceId, offer_id: p.offerId, offer_kind: 'compute.execution.v1', provider_id: PUBLIC_DEMO.providerId, runtime: 'wasm', package_kind: 'inline_module', entrypoint_kind: 'module', entrypoint: p.entrypoint, contract_version: 'froglet.wasm.host_json.v1', mode: 'sync', publication_state: 'active', binding_hash: p.bindingHash, module_hash: p.moduleHash, capabilities: [cap], mounts: [], base_fee_msat: 0, success_fee_msat: 0, settlement_method: 'none' };
  const revision = { revision_hash: p.revisionHash, signer_pubkey: PUBLIC_DEMO.providerId, payload: { provider_id: PUBLIC_DEMO.providerId, service_id: serviceId, offer_id: p.offerId, offer_hash: p.offerHash, binding_hash: p.bindingHash, runtime: 'wasm', package_kind: 'inline_module', service: { entrypoint_kind: 'module', entrypoint: p.entrypoint, contract_version: service.contract_version, capabilities: [cap], mounts: [] } } };
  const seen: any[] = [], publication = vi.fn(() => ({ valid: true }));
  let quote: any, record: any;
  const deps: any = { kernel, providerId: PUBLIC_DEMO.providerId, publishedServices: profiles, verifyPublication: publication, verifier: { verifyDocument: () => ({ status: 'verified' }), validateChain: () => ({ valid: true, chain_evaluated: true }) }, transport: async (request: any) => {
    seen.push(structuredClone(request));
    let status = 200, body: any;
    if (request.path === '/v1/provider/descriptor') body = structuredClone(descriptor);
    else if (request.path === '/v1/provider/offers') body = { offers: [structuredClone(offer)] };
    else if (request.path === '/v1/provider/services/' + serviceId) body = { service: structuredClone(service), publication_revision: structuredClone(revision) };
    else if (request.path === '/v1/provider/quotes') {
      status = 201; quote = body = { artifact_type: 'quote', hash: 'e'.repeat(64), payload: { provider_id: PUBLIC_DEMO.providerId, requester_id: request.body.requester_id, descriptor_hash: p.descriptorHash, offer_hash: p.offerHash, workload_kind: 'compute.execution.v1', workload_hash: kernel.hashJson(request.body.execution), expires_at: now + 60, capabilities_granted: [cap], execution_limits: limits, settlement_terms: { method: 'none', base_fee_msat: 0, success_fee_msat: 0 } } };
    } else if (request.path === '/v1/provider/deals') {
      const result = serviceId === 'marketplace-provider' ? { provider: null } : { items: [], pagination: { limit: 1, offset: 0, total: 0 } };
      record = body = { deal_id: 'named-fixture', status: 'succeeded', deal: request.body.deal, quote, result, result_hash: kernel.hashJson(result), receipt: { artifact_type: 'receipt', hash: 'f'.repeat(64), payload: { provider_id: PUBLIC_DEMO.providerId, requester_id: request.body.deal.payload.requester_id, quote_hash: quote.hash, deal_hash: request.body.deal.hash, deal_state: 'succeeded', result_hash: kernel.hashJson(result), executor: { runtime: 'wasm', abi_version: 'froglet.wasm.host_json.v1', module_hash: p.moduleHash, capabilities_granted: [cap] } } } };
    } else if (request.path === '/v1/provider/deals/named-fixture') body = record;
    else throw new Error('unexpected fixture request ' + request.path);
    alter(body, request.path); return { status, body };
  } };
  return { deps, seen, p, cap, publication };
}

test.each(ids)('%s uses the pinned published module and exact operation grant, never caller-supplied bytes', async serviceId => {
  const f = fixture(serviceId);
  const input = { provider_id: '0'.repeat(64), ...(serviceId === 'marketplace-provider' ? {} : { limit: 1 }) };
  const run = await prepare(f.deps, serviceId, JSON.stringify(input));
  const sent = f.seen.find(r => r.path.endsWith('/quotes')).body;
  expect(sent.kind).toBe('execution'); expect(sent.submission).toBeUndefined();
  expect(sent.execution).toEqual(expect.objectContaining({ runtime: 'wasm', package_kind: 'inline_module', module_hash: f.p.bindingHash, requested_access: [f.cap], security: { mode: 'standard', service_id: serviceId } }));
  expect(f.publication).toHaveBeenCalledOnce();
  const outcome = await client.resumeLiveRun(f.deps, run); expect(outcome.terminal).toBe(true);
  if (outcome.terminal) { expect(outcome.status).toBe('succeeded'); expect(client.exportLiveEvidence(outcome)).toHaveProperty('publication_revision.revision_hash', f.p.revisionHash); }
  const posts = f.seen.filter(r => r.method === 'POST').length;
  expect((await client.resumeLiveRun(f.deps, JSON.parse(JSON.stringify(run)))).terminal).toBe(true);
  expect(f.seen.filter(r => r.method === 'POST')).toHaveLength(posts);
});

test.each(['events.query', 'marketplace.provider', 'execute.compute.generic', 'https://other.invalid', 'other'])('unsupported named selection %s refuses before any transport', async id => {
  const f = fixture(); await expect(prepare(f.deps, id, '{}')).rejects.toThrow(); expect(f.seen).toHaveLength(0);
});

test.each(['binding_hash', 'module_hash', 'contract_version', 'entrypoint', 'provider_id', 'capabilities'])('altered native service %s fails before quote', async field => {
  const f = fixture('marketplace-provider', (body, path) => { if (path.includes('/services/')) body.service[field] = field === 'capabilities' ? ['net.http.fetch'] : 'altered'; });
  await expect(prepare(f.deps, 'marketplace-provider', JSON.stringify({ provider_id: '0'.repeat(64) }))).rejects.toThrow();
  expect(f.seen.some(r => r.path.endsWith('/quotes'))).toBe(false);
});

test('an invalid publication verdict, expired quote, extra grant or changed result cannot become verified success', async () => {
  const unsigned = fixture(); unsigned.publication.mockReturnValue({ valid: false });
  await expect(prepare(unsigned.deps, 'marketplace-provider', JSON.stringify({ provider_id: '0'.repeat(64) }))).rejects.toThrow();
  for (const kind of ['expired', 'grant', 'result']) {
    const f = fixture('marketplace-provider', (body, path) => {
      if (path.endsWith('/quotes') && kind === 'expired') body.payload.expires_at = 1;
      if (path.endsWith('/quotes') && kind === 'grant') body.payload.capabilities_granted.push('net.http.fetch');
      if (path.endsWith('/deals') && kind === 'result') body.result = { provider: 'changed' };
    });
    if (kind === 'result') { const run = await prepare(f.deps, 'marketplace-provider', JSON.stringify({ provider_id: '0'.repeat(64) })); expect((await client.resumeLiveRun(f.deps, run)).terminal).toBe(false); }
    else await expect(prepare(f.deps, 'marketplace-provider', JSON.stringify({ provider_id: '0'.repeat(64) }))).rejects.toThrow();
  }
});

test.each([{ url: 'https://other.invalid' }, { provider_id: 'UPPER'.repeat(13) }, { provider_id: '0'.repeat(64), token: 'private' }, { provider_id: '0'.repeat(64), limit: 21 }])('invalid named input %j fails before transport', async input => {
  const f = fixture(); await expect(prepare(f.deps, 'marketplace-provider', JSON.stringify(input))).rejects.toThrow(); expect(f.seen).toHaveLength(0);
});

test.each(['/v1/provider/descriptor', '/v1/provider/offers'])('expired discovery at%s is not used to request a quote', async path => {
  const f = fixture('marketplace-provider', (body, at) => { if (at === path) (body.payload ?? body.offers[0].payload).expires_at = 1; });
  await expect(prepare(f.deps, 'marketplace-provider', JSON.stringify({ provider_id: '0'.repeat(64) }))).rejects.toThrow(); expect(f.seen.some(r => r.path.endsWith('/quotes'))).toBe(false);
});

test('accepted historical named results recover without current enabling pins, while unsubmitted work cannot', async () => {
  const f = fixture(), input = JSON.stringify({ provider_id: '0'.repeat(64) });
  const unsubmitted = await prepare(f.deps, 'marketplace-provider', input);
  await expect(client.resumeLiveRun({ ...f.deps, publishedServices: [] }, structuredClone(unsubmitted))).rejects.toThrow('no longer enabled');
  expect((await client.resumeLiveRun(f.deps, unsubmitted)).terminal).toBe(true);
  const posts = f.seen.filter(r => r.method === 'POST').length;
  expect((await client.resumeLiveRun({ ...f.deps, publishedServices: [] }, structuredClone(unsubmitted))).terminal).toBe(true);
  expect(f.seen.filter(r => r.method === 'POST')).toHaveLength(posts);
});

test.each(['module', 'capabilities'])('a signed-shaped receipt reporting different%s cannot verify', async kind => {
  const f = fixture('marketplace-provider', (body, path) => { if (path.endsWith('/deals')) { if (kind === 'module') body.receipt.payload.executor.module_hash = '0'.repeat(64); else body.receipt.payload.executor.capabilities_granted = ['net.http.fetch']; } });
  const run = await prepare(f.deps, 'marketplace-provider', JSON.stringify({ provider_id: '0'.repeat(64) }));
  expect((await client.resumeLiveRun(f.deps, run)).terminal).toBe(false);
});

test('recovery binds the saved publication revision to its selected service profile before transport', async () => {
  const f = fixture(), run = await prepare(f.deps, 'marketplace-provider', JSON.stringify({ provider_id: '0'.repeat(64) }));
  await client.resumeLiveRun(f.deps, run);
  const reads = f.seen.length;
  for (const key of ['revision_hash', 'service_id', 'binding_hash']) {
    const altered = structuredClone(run), revision = altered.publishedService!.revision as any;
    if (key === 'revision_hash') revision[key] = 'a'.repeat(64);
    else revision.payload[key] = 'a'.repeat(64);
    await expect(client.resumeLiveRun({ ...f.deps, publishedServices: [] }, altered)).rejects.toThrow();
    expect(f.seen).toHaveLength(reads);
  }
});

test('changing only the saved displayed input cannot recover or export a different named request', async () => {
  const f = fixture(), run = await prepare(f.deps, 'marketplace-provider', JSON.stringify({ provider_id: '0'.repeat(64) }));
  await client.resumeLiveRun(f.deps, run);
  const reads = f.seen.length, altered = structuredClone(run);
  altered.input = { provider_id: 'a'.repeat(64) };
  await expect(client.resumeLiveRun(f.deps, altered)).rejects.toThrow('saved input has changed');
  expect(f.seen).toHaveLength(reads);
});

test('a named operation failure exports its verified failure chain without a result claim', async () => {
  const f = fixture('marketplace-provider', (body, path) => {
    if (path.endsWith('/deals')) {
      body.status = body.receipt.payload.deal_state = 'failed';
      body.receipt.payload.failure_code = 'execution_timed_out';
      delete body.result; delete body.result_hash; delete body.receipt.payload.result_hash;
    }
  });
  const run = await prepare(f.deps, 'marketplace-provider', JSON.stringify({ provider_id: '0'.repeat(64) }));
  const outcome = await client.resumeLiveRun(f.deps, run);
  expect(outcome).toEqual(expect.objectContaining({ terminal: true, status: 'failed', failure: 'execution_timed_out' }));
  if (outcome.terminal) {
    const evidence = client.exportLiveEvidence(outcome);
    expect(evidence.result).toBeNull(); expect(evidence.artifacts).toHaveLength(5);
    expect(evidence.artifacts[4].payload.result_hash).toBeUndefined();
  }
});
