// @vitest-environment node
import { afterEach, expect, test, vi } from 'vitest';
// The unrelated service-link route imports Worker-specific Wasm. This test exercises HTTP routing only.
vi.mock('../../data/service-link-verifier', () => ({ verifyServiceLinkEvidence: () => ({ valid: true }) }));
import worker from '../../worker';
import { PUBLIC_DEMO, PUBLIC_MARKETPLACE_READ } from '../../data/public-demo-config';
import { kernel, verifier } from './playground-helpers';

afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

const env = (extra: Record<string, unknown> = {}) => ({ ASSETS: { fetch: async () => new Response('Not found', { status: 404 }) }, FROGLET_PUBLIC_DEMO_ENABLED: 'true', PUBLIC_DEMO_RATE_LIMITER: { limit: async () => ({ success: true }) }, ...extra });
const request = (path: string, method = 'GET', body?: unknown, extra: Record<string, string> = {}) => new Request('https://froglet.dev/api/public-demo' + path, { method, headers: { ...(body === undefined ? {} : { 'content-type': 'application/json' }), 'cf-connecting-ip': '192.0.2.1', ...extra }, body: body === undefined ? undefined : JSON.stringify(body) });

test('the public demo reaches its provider without an invitation or operator credential', async () => {
  const upstream = vi.fn(async () => new Response(JSON.stringify({ accepted: true }), { status: 201, headers: { 'content-type': 'application/json' } }));
  vi.stubGlobal('fetch', upstream);
  const request = new Request('https://froglet.dev/api/public-demo/v1/provider/quotes', {
    method: 'POST', headers: { 'content-type': 'application/json', 'cf-connecting-ip': '192.0.2.1' },
    body: JSON.stringify({ offer_id: 'execute.compute', requester_id: 'a'.repeat(64), kind: 'wasm', max_price_sats: 0, submission: {
      schema_version: 'froglet/v1', submission_type: 'wasm_submission', module_bytes_hex: '0061736d01000000', input: { a: 6, b: 7 },
      workload: { schema_version: 'froglet/v1', workload_kind: 'compute.wasm.v1', abi_version: 'froglet.wasm.run_json.v1', module_format: 'application/wasm', module_hash: 'b'.repeat(64), input_format: 'application/json+jcs', input_hash: 'c'.repeat(64), requested_capabilities: [] },
    } }),
  });
  const response = await worker.fetch(request, {
    ASSETS: { fetch: async () => new Response('Not found', { status: 404 }) },
    FROGLET_PUBLIC_DEMO_ENABLED: 'true',
    PUBLIC_DEMO_RATE_LIMITER: { limit: async () => ({ success: true }) },
  } as any);
  expect(response.status).toBe(201);
  expect(upstream).toHaveBeenCalledOnce();
  const forwarded = upstream.mock.calls[0][0] as Request;
  expect(forwarded.headers.has('authorization')).toBe(false);
  expect(forwarded.headers.has('cookie')).toBe(false);
  expect(forwarded.headers.has('x-froglet-access-token')).toBe(false);
  expect(response.headers.get('cache-control')).toBe('no-store');
});

test('disabled or missing admission controls fail closed without reaching a provider', async () => {
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  for (const settings of [env({ FROGLET_PUBLIC_DEMO_ENABLED: 'false' }), env({ PUBLIC_DEMO_RATE_LIMITER: undefined }), env({ FROGLET_PUBLIC_DEMO_ORIGIN: 'http://169.254.169.254' }), env({ FROGLET_PUBLIC_DEMO_PROVIDER_ID: 'not-an-identity' })]) {
    const response = await worker.fetch(request('/health'), settings as any);
    expect(response.status).toBe(503);
  }
  expect(upstream).not.toHaveBeenCalled();
});

test('operator, runtime, legacy and caller-selected URLs are never forwarded', async () => {
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  for (const path of ['/v1/runtime/deals', '/v1/provider/artifacts/publish', '/v1/provider/usage', '/v1/node/execute/wasm', '/https://example.com', '/v1/provider/descriptor?token=private', '/v1/provider/%64escriptor']) {
    const response = await worker.fetch(request(path), env() as any);
    expect([400, 404]).toContain(response.status);
  }
  expect(upstream).not.toHaveBeenCalled();
});

test('an exact signed artifact is read from the fixed provider and must match the requested hash', async () => {
  const hash = 'd'.repeat(64);
  const upstream = vi.fn(async () => new Response(JSON.stringify({ hash, kind: 'descriptor' })));
  vi.stubGlobal('fetch', upstream);
  expect((await worker.fetch(request('/v1/artifacts/' + hash), env() as any)).status).toBe(200);
  expect((upstream.mock.calls[0][0] as Request).url).toBe(PUBLIC_DEMO.origin + '/v1/artifacts/' + hash);
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ hash: 'e'.repeat(64), kind: 'descriptor' }))));
  expect((await worker.fetch(request('/v1/artifacts/' + hash), env() as any)).status).toBe(502);
  const refused = vi.fn(); vi.stubGlobal('fetch', refused);
  for (const path of ['/v1/artifacts/' + 'D'.repeat(64), '/v1/artifacts/' + 'd'.repeat(63), '/v1/artifacts/' + hash + '/raw', '/v1/artifacts']) {
    expect((await worker.fetch(request(path), env() as any)).status).toBe(404);
  }
  expect((await worker.fetch(request('/v1/artifacts/' + hash, 'POST', {}), env() as any)).status).toBe(404);
  expect(refused).not.toHaveBeenCalled();
});

test('cross-site browser requests and exhausted edge rate limit are refused', async () => {
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  expect((await worker.fetch(request('/health', 'GET', undefined, { origin: 'https://other.invalid' }), env() as any)).status).toBe(403);
  const response = await worker.fetch(request('/health'), env({ PUBLIC_DEMO_RATE_LIMITER: { limit: async () => ({ success: false }) } }) as any);
  expect(response.status).toBe(429);
  expect(response.headers.get('retry-after')).toBe('10');
  expect(upstream).not.toHaveBeenCalled();
});

test('provider metadata configuration returns only public values and no network call', async () => {
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  const response = await worker.fetch(request('/config'), env() as any);
  expect(response.status).toBe(200);
  const config = await response.json() as any;
  expect(config.providerId).toBe(PUBLIC_DEMO.providerId);
  expect(config.providerOrigin).toBe(PUBLIC_DEMO.origin);
  expect(JSON.stringify(config)).not.toContain('token');
  expect(upstream).not.toHaveBeenCalled();
});

test('upstream redirects, non-JSON and oversize responses cannot escape the fixed provider boundary', async () => {
  for (const value of [new Response(null, { status: 302, headers: { location: 'https://other.invalid' } }), new Response('not JSON'), new Response('"' + 'x'.repeat(1048576) + '"')]) {
    vi.stubGlobal('fetch', vi.fn(async () => value));
    const response = await worker.fetch(request('/health'), env() as any);
    expect(response.status).toBe(502);
    expect(response.headers.get('cache-control')).toBe('no-store');
  }
});

test('client credentials and upstream cookies are excluded from every response', async () => {
  const upstream = vi.fn(async () => new Response('{"status":"ok"}', { headers: { 'set-cookie': 'private=secret', 'content-type': 'application/json' } }));
  vi.stubGlobal('fetch', upstream);
  const response = await worker.fetch(request('/health', 'GET', undefined, { authorization: 'Bearer private', cookie: 'private=secret', 'x-froglet-access-token': 'private' }), env() as any);
  const forwarded = upstream.mock.calls[0][0] as Request;
  for (const header of ['authorization', 'cookie', 'x-froglet-access-token']) expect(forwarded.headers.has(header)).toBe(false);
  expect(response.headers.has('set-cookie')).toBe(false);
});

test('other execution types, fees, and host capabilities do not reach the node', async () => {
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  for (const body of [{ kind: 'oci_wasm' }, { kind: 'events_query' }, { kind: 'execution', execution: { runtime: 'python' } }, { kind: 'wasm', max_price_sats: 1 }, { kind: 'wasm', submission: { workload: { requested_capabilities: ['net.http.fetch'] } } }]) {
    expect((await worker.fetch(request('/v1/provider/quotes', 'POST', body), env() as any)).status).toBe(400);
  }
  expect(upstream).not.toHaveBeenCalled();
});

const readProvider = 'c7a15140cc28833978197bdef7daf30e419502f186f59ff88584f453531ea894';
const absentProvider = '0'.repeat(64);
const readEnv = (extra: Record<string, unknown> = {}) => env({ FROGLET_PUBLIC_MARKETPLACE_READ_ENABLED: 'true', ...extra });
const readRequest = (operation: string, body: unknown, headers: Record<string, string> = {}) => new Request('https://froglet.dev/api/marketplace-read/' + operation, { method: 'POST', headers: { 'content-type': 'application/json', 'cf-connecting-ip': '192.0.2.1', ...headers }, body: JSON.stringify(body) });
const providerRead = () => ({ provider_id: readProvider, current_descriptor_hash: 'b'.repeat(64), descriptor: { protocol_version: 'froglet/v1', service_kinds: ['compute.wasm.v1'], execution_runtimes: ['wasm'], transport_endpoints: [{ transport: 'https', uri: 'https://fixture.invalid', priority: 1 }] }, trust: { success_count: 2, failure_count: 1 }, ignored_private_field: 'must not be forwarded' });

test('the three read adapters are disabled by default and require the existing edge limiter', async () => {
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  for (const settings of [env(), readEnv({ PUBLIC_DEMO_RATE_LIMITER: undefined }), readEnv({ FROGLET_PUBLIC_MARKETPLACE_READ_ENABLED: 'false' })]) {
    expect((await worker.fetch(readRequest('provider', { provider_id: readProvider }), settings as any)).status).toBe(503);
  }
  expect(upstream).not.toHaveBeenCalled();
});

test('provider reads use one fixed public GET and return only the declared public fields', async () => {
  const upstream = vi.fn(async () => new Response(JSON.stringify(providerRead()), { headers: { 'set-cookie': 'private=secret' } })); vi.stubGlobal('fetch', upstream);
  const response = await worker.fetch(readRequest('provider', { provider_id: readProvider }, { authorization: 'Bearer private', cookie: 'private=secret', 'x-froglet-access-token': 'private' }), readEnv() as any);
  expect(response.status).toBe(200);
  expect(upstream).toHaveBeenCalledOnce();
  const forwarded = upstream.mock.calls[0][0] as Request;
  expect(forwarded.url).toBe('https://marketplace.froglet.dev/v1/providers/' + readProvider);
  expect(forwarded.method).toBe('GET');
  expect(forwarded.redirect).toBe('manual');
  for (const name of ['authorization', 'cookie', 'x-froglet-access-token']) expect(forwarded.headers.has(name)).toBe(false);
  expect(response.headers.has('set-cookie')).toBe(false);
  expect(await response.json()).toEqual({ provider: { provider_id: readProvider, current_descriptor_hash: 'b'.repeat(64), protocol_version: 'froglet/v1', service_kinds: ['compute.wasm.v1'], execution_runtimes: ['wasm'], transport_endpoints: [{ transport: 'https', uri: 'https://fixture.invalid' }], success_count: 2, failure_count: 1 } });
});

test.each(['search', 'receipts'])('%s has a canonical bounded empty result without generated timestamps', async operation => {
  const upstream = vi.fn(async () => new Response(JSON.stringify({ items: [], pagination: { limit: 1, offset: 0, total: 0 } }))); vi.stubGlobal('fetch', upstream);
  const response = await worker.fetch(readRequest(operation, { provider_id: absentProvider, limit: 1 }), readEnv() as any);
  expect(response.status).toBe(200);
  expect(await response.json()).toEqual({ items: [], pagination: { limit: 1, offset: 0, total: 0 } });
  expect(upstream).toHaveBeenCalledOnce();
  const forwarded = upstream.mock.calls[0][0] as Request;
  const target = new URL(forwarded.url);
  expect(target.origin).toBe('https://marketplace.froglet.dev');
  expect(target.pathname).toBe(operation === 'search' ? '/v1/offers' : '/v1/providers/' + absentProvider + '/receipts');
  expect(target.searchParams.get('limit')).toBe('1');
  expect(target.searchParams.get('offset')).toBe('0');
  expect(forwarded.method).toBe('GET');
});

test('a nonexistent provider maps the existing API404 to a stable nullable result', async () => {
  vi.stubGlobal('fetch', vi.fn(async () => new Response('{"error":"not found"}', { status: 404 })));
  const response = await worker.fetch(readRequest('provider', { provider_id: absentProvider }), readEnv() as any);
  expect(response.status).toBe(200);
  expect(await response.json()).toEqual({ provider: null });
});

test.each([
  ['provider', {}], ['provider', { provider_id: readProvider, token: 'private' }],
  ['provider', { provider_id: 'https://127.0.0.1' }], ['provider', { provider_id: readProvider.toUpperCase() }],
  ['search', { max_price_sats: 0 }], ['search', { runtime: 'container' }], ['search', { limit: 0 }], ['search', { limit: 21 }],
  ['search', { offset: 1001 }], ['search', { offer_kind: '../admin' }], ['search', { availability: 'ready' }],
  ['receipts', { provider_id: readProvider, status: 'succeeded' }], ['receipts', { provider_id: readProvider, cursor: '0' }],
  ['receipts', { provider_id: readProvider, updated_since: -1 }], ['search', []], ['provider', null],
])('unsupported %s input is refused before network use: %j', async (operation, input) => {
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  expect((await worker.fetch(readRequest(operation as string, input), readEnv() as any)).status).toBe(400);
  expect(upstream).not.toHaveBeenCalled();
});

test('read adapters refuse URL query, extra paths, wrong methods, and cross-site browser calls', async () => {
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  for (const suffix of ['provider?url=https://evil.invalid', 'provider/extra', 'events', 'provider%2fextra']) {
    expect([400, 404]).toContain((await worker.fetch(readRequest(suffix, { provider_id: readProvider }), readEnv() as any)).status);
  }
  expect((await worker.fetch(new Request('https://froglet.dev/api/marketplace-read/provider'), readEnv() as any)).status).toBe(405);
  expect((await worker.fetch(readRequest('provider', { provider_id: readProvider }, { origin: 'https://other.invalid' }), readEnv() as any)).status).toBe(403);
  expect(upstream).not.toHaveBeenCalled();
});

test('the read adapter applies request, response, rate and upstream failure boundaries', async () => {
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  expect((await worker.fetch(readRequest('search', { offer_kind: 'x'.repeat(2048) }), readEnv() as any)).status).toBe(413);
  expect((await worker.fetch(readRequest('search', {}), readEnv({ PUBLIC_DEMO_RATE_LIMITER: { limit: async () => ({ success: false }) } }) as any)).status).toBe(429);
  expect(upstream).not.toHaveBeenCalled();
  for (const response of [new Response(null, { status: 302, headers: { location: 'https://evil.invalid' } }), new Response('not JSON'), new Response('"' + 'x'.repeat(131072) + '"'), new Response('{"private":"sql diagnostic"}', { status: 500 })]) {
    upstream.mockResolvedValueOnce(response);
    const answer = await worker.fetch(readRequest('provider', { provider_id: readProvider }), readEnv() as any);
    expect(answer.status).toBe(502);
    expect(await answer.text()).not.toContain('sql diagnostic');
  }
});

test('malformed or mismatched public JSON cannot satisfy an adapter output contract', async () => {
  const badProvider = providerRead(); badProvider.provider_id = 'f'.repeat(64);
  for (const [operation, input, body] of [
    ['provider', { provider_id: readProvider }, badProvider],
    ['provider', { provider_id: readProvider }, { ...providerRead(), trust: { success_count: Number.MAX_SAFE_INTEGER + 1, failure_count: 0 } }],
    ['search', { limit: 1 }, { items: [], pagination: { limit: 20, offset: 0, total: 0 } }],
    ['receipts', { provider_id: readProvider }, { items: [], pagination: { limit: 10, offset: 0, total: -1 } }],
  ] as const) {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(body))));
    expect((await worker.fetch(readRequest(operation, input), readEnv() as any)).status).toBe(502);
  }
});

const offerRead = () => ({ artifact_hash: 'a'.repeat(64), provider_id: readProvider, offer_id: 'synthetic-terminology-demo', offer_kind: 'synthetic-terminology-demo', runtime: 'builtin', package_kind: 'builtin', contract_version: 'froglet.builtin.data_query.json.v1', settlement_method: 'none', base_fee_msat: 0, success_fee_msat: 0, availability: { status: 'unknown', admission: 'unknown', lease_expires_at: null, last_renewed_at: null }, unknown_field: 'omit me' });

test('positive search and receipt reads project bounded rows without claiming inner-record signatures', async () => {
  const receipt = { artifact_hash: 'd'.repeat(64), provider_id: readProvider, deal_hash: 'e'.repeat(64), deal_state: 'succeeded', execution_state: 'succeeded', runtime: 'wasm', finished_at: 1700000000, failure_message: 'omit me' };
  for (const [operation, row, expected] of [
    ['search', offerRead(), { ...offerRead(), unknown_field: undefined }],
    ['receipts', receipt, { artifact_hash: receipt.artifact_hash, provider_id: readProvider, deal_hash: receipt.deal_hash, status: 'succeeded', execution_state: 'succeeded', runtime: 'wasm', finished_at: receipt.finished_at }],
  ] as const) {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ items: [row], pagination: { limit: 1, offset: 0, total: 1 } }))));
    const response = await worker.fetch(readRequest(operation, { provider_id: readProvider, limit: 1 }), readEnv() as any);
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ items: [JSON.parse(JSON.stringify(expected))], pagination: { limit: 1, offset: 0, total: 1 } });
  }
});

test('supported search filters are encoded on the fixed offer read without extra provider calls', async () => {
  const upstream = vi.fn(async () => new Response('{"items":[],"pagination":{"limit":20,"offset":1000,"total":0}}')); vi.stubGlobal('fetch', upstream);
  const response = await worker.fetch(readRequest('search', { provider_id: readProvider, offer_kind: 'marketplace.search', runtime: 'builtin', availability: 'healthy', limit: 20, offset: 1000 }), readEnv() as any);
  expect(response.status).toBe(200); expect(upstream).toHaveBeenCalledOnce();
  const target = new URL((upstream.mock.calls[0][0] as Request).url);
  expect(target.origin + target.pathname).toBe('https://marketplace.froglet.dev/v1/offers');
  expect(Object.fromEntries(target.searchParams)).toEqual({ provider_id: readProvider, offer_kind: 'marketplace.search', runtime: 'builtin', availability: 'healthy', limit: '20', offset: '1000' });
});

test('read bodies enforce UTF-8 byte ceilings and reject malformed JSON/media before upstream use', async () => {
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  for (const [body, contentType, status] of [['{', 'application/json', 400], ['{}', 'text/plain', 415], [JSON.stringify({ offer_kind: '€'.repeat(800) }), 'application/json', 413]] as const) {
    const response = await worker.fetch(new Request('https://froglet.dev/api/marketplace-read/search', { method: 'POST', headers: { 'content-type': contentType }, body }), readEnv() as any);
    expect(response.status).toBe(status);
  }
  expect(upstream).not.toHaveBeenCalled();
});

test('a stalled request body is cancelled within the total one-second read deadline', async () => {
  vi.useFakeTimers();
  const cancelled = vi.fn(), upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  const body = new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(new TextEncoder().encode('{')); }, cancel: cancelled });
  const request = new Request('https://froglet.dev/api/marketplace-read/search', { method: 'POST', headers: { 'content-type': 'application/json' }, body, duplex: 'half' } as RequestInit);
  const response = worker.fetch(request, readEnv() as any);
  await vi.advanceTimersByTimeAsync(1000);
  expect((await response).status).toBe(408);
  expect(cancelled).toHaveBeenCalledOnce(); expect(upstream).not.toHaveBeenCalled();
});

test('a stalled or oversized upstream stream is cancelled and never leaks a partial result', async () => {
  vi.useFakeTimers();
  const cancelled = vi.fn();
  vi.stubGlobal('fetch', vi.fn(async () => new Response(new ReadableStream({ start(controller) { controller.enqueue(new TextEncoder().encode('{')); }, cancel: cancelled }))));
  const response = worker.fetch(readRequest('provider', { provider_id: readProvider }), readEnv() as any);
  await vi.advanceTimersByTimeAsync(1000);
  expect((await response).status).toBe(502); expect(cancelled).toHaveBeenCalledOnce();
  vi.useRealTimers();
  const oversizedCancelled = vi.fn();
  vi.stubGlobal('fetch', vi.fn(async () => new Response(new ReadableStream({ start(controller) { controller.enqueue(new Uint8Array(131073)); }, cancel: oversizedCancelled }))));
  expect((await worker.fetch(readRequest('provider', { provider_id: readProvider }), readEnv() as any)).status).toBe(502);
  expect(oversizedCancelled).toHaveBeenCalledOnce();
});

test('the adapter reports depleted API capacity and admission failures without private error content', async () => {
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  expect((await worker.fetch(readRequest('search', {}), readEnv({ PUBLIC_DEMO_RATE_LIMITER: { limit: async () => { throw new Error('private binding diagnostic'); } } }) as any)).status).toBe(503);
  expect(upstream).not.toHaveBeenCalled();
  upstream.mockResolvedValueOnce(new Response('{"private":"not for the browser"}', { status: 429, headers: { 'set-cookie': 'private=secret' } }));
  const response = await worker.fetch(readRequest('search', {}), readEnv() as any);
  expect(response.status).toBe(429); expect(response.headers.get('retry-after')).toBe('10');
  expect(await response.text()).not.toContain('not for the browser'); expect(response.headers.has('set-cookie')).toBe(false);
});

test('real elapsed deadline bounds hanging admission, fetch and cancellation without later network work', async () => {
  const calls: string[] = [], cancelled: string[] = [];
  const never = () => new Promise<never>(() => {});
  const uncooperativeBody = (name: string) => new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(new TextEncoder().encode('{')); }, cancel() { cancelled.push(name); return never(); } });
  vi.stubGlobal('fetch', vi.fn(async (request: Request) => {
    const id = new URL(request.url).pathname.split('/').at(-1)!; calls.push(id);
    if (id === 'a'.repeat(64)) return new Response(uncooperativeBody('404'), { status: 404 });
    if (id === 'b'.repeat(64)) return new Response(uncooperativeBody('500'), { status: 500 });
    return never(); // Deliberately ignores AbortSignal to test the absolute response boundary.
  }));
  let grantLate: ((value: { success: boolean }) => void) | undefined;
  const late = new Promise<{ success: boolean }>(resolve => { grantLate = resolve; });
  const limiter = { limit: async ({ key }: { key: string }) => key.endsWith('hanging') ? never() : key.endsWith('late') ? late : { success: true } };
  const stalledRequest = new Request('https://froglet.dev/api/marketplace-read/search', { method: 'POST', headers: { 'content-type': 'application/json' }, body: uncooperativeBody('request'), duplex: 'half' } as RequestInit);
  const cases = [
    ['hanging admission', readRequest('provider', { provider_id: 'd'.repeat(64) }, { 'cf-connecting-ip': 'hanging' }), 503],
    ['late admission', readRequest('provider', { provider_id: 'e'.repeat(64) }, { 'cf-connecting-ip': 'late' }), 503],
    ['request cancellation', stalledRequest, 408],
    ['404 cancellation', readRequest('provider', { provider_id: 'a'.repeat(64) }), 200],
    ['error cancellation', readRequest('provider', { provider_id: 'b'.repeat(64) }), 502],
    ['upstream headers', readRequest('provider', { provider_id: 'c'.repeat(64) }), 502],
  ] as const;
  const observed = await Promise.all(cases.map(async ([name, request, expected]) => {
    const started = performance.now();
    const answer = worker.fetch(request, readEnv({ PUBLIC_DEMO_RATE_LIMITER: limiter }) as any);
    let cutoff: ReturnType<typeof setTimeout> | undefined;
    const result = await Promise.race([
      answer.then(response => ({ settled: true, status: response.status })),
      new Promise<{ settled: false; status: null }>(resolve => { cutoff = setTimeout(() => resolve({ settled: false, status: null }), 1400); }),
    ]);
    if (cutoff) clearTimeout(cutoff);
    return { name, expected, ...result, elapsed: performance.now() - started };
  }));
  grantLate!({ success: true });
  await new Promise(resolve => setTimeout(resolve, 20));
  expect(observed, JSON.stringify(observed)).toEqual(expect.arrayContaining(cases.map(([name, _request, expected]) => expect.objectContaining({ name, settled: true, status: expected }))));
  expect(calls).not.toContain('d'.repeat(64)); expect(calls).not.toContain('e'.repeat(64));
  expect(cancelled).toEqual(expect.arrayContaining(['request', '404', '500']));
}, 5000);

// These dummy provider envelopes test Worker routing/field policy only. The
// native node remains authoritative for signatures and workload commitments.
// No fixture profile is a production pin or a native execution qualification.
const namedIds = ['marketplace-provider', 'marketplace-search', 'marketplace-receipts'] as const;
const namedProfiles = namedIds.map((serviceId, i) => ({ serviceId, offerId: serviceId, offerHash: String(i + 1).repeat(64), bindingHash: String(i + 4).repeat(64), moduleHash: String(i + 4).repeat(64), revisionHash: String(i + 7).repeat(64), descriptorHash: 'd'.repeat(64), operationHash: ['a', 'b', 'c'][i].repeat(64), entrypoint: 'run' }));
const namedEnv = (extra: Record<string, unknown> = {}) => env({ FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_ENABLED: 'true', FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_JSON: JSON.stringify({ providerId: PUBLIC_DEMO.providerId, profiles: namedProfiles }), ...extra });
function namedWork(serviceId: typeof namedIds[number] = 'marketplace-provider') {
  const p = namedProfiles.find(p => p.serviceId === serviceId)!;
  return { schema_version: 'froglet/v1', workload_kind: 'compute.execution.v1', runtime: 'wasm', package_kind: 'inline_module', entrypoint: { kind: 'module', value: 'run' }, contract_version: 'froglet.wasm.host_json.v1', input_format: 'application/json+jcs', input_hash: 'e'.repeat(64), input: { provider_id: '0'.repeat(64) }, module_hash: p.bindingHash, security: { mode: 'standard', service_id: serviceId }, requested_access: ['net.http.operation.' + p.operationHash] };
}
function namedQuote(p = namedProfiles[0]) {
  return { schema_version: 'froglet/v1', artifact_type: 'quote', created_at: Math.floor(Date.now() / 1000), hash: 'e'.repeat(64), payload_hash: 'f'.repeat(64), signer: PUBLIC_DEMO.providerId, signature: '1'.repeat(128), payload: { provider_id: PUBLIC_DEMO.providerId, requester_id: 'a'.repeat(64), descriptor_hash: p.descriptorHash, offer_hash: p.offerHash, expires_at: Math.floor(Date.now() / 1000) + 30, workload_kind: 'compute.execution.v1', workload_hash: 'b'.repeat(64), capabilities_granted: ['net.http.operation.' + p.operationHash], settlement_terms: { method: 'none', destination_identity: '', base_fee_msat: 0, success_fee_msat: 0, max_base_invoice_expiry_secs: 0, max_success_hold_expiry_secs: 0, min_final_cltv_expiry: 0 }, execution_limits: { max_runtime_ms: 2000, max_memory_bytes: 8388608, fuel_limit: 50000000, max_input_bytes: 2048, max_output_bytes: 131072 } } };
}
const namedQuoteBody = (id: typeof namedIds[number] = 'marketplace-provider') => ({ offer_id: id, requester_id: 'a'.repeat(64), kind: 'execution', execution: namedWork(id), max_price_sats: 0 });
function namedDealBody() {
  const quote = namedQuote(), q = quote.payload;
  return { kind: 'execution', execution: namedWork(), quote, idempotency_key: 'c'.repeat(32), deal: { schema_version: 'froglet/v1', artifact_type: 'deal', created_at: quote.created_at, hash: 'd'.repeat(64), payload_hash: 'e'.repeat(64), signature: '2'.repeat(128), signer: q.requester_id, payload: { provider_id: q.provider_id, requester_id: q.requester_id, quote_hash: quote.hash, workload_hash: q.workload_hash, success_payment_hash: 'f'.repeat(64), admission_deadline: q.expires_at, completion_deadline: q.expires_at + 2, acceptance_deadline: q.expires_at + 2 } } };
}
function namedMetadata(p = namedProfiles[0]) {
  const operation = PUBLIC_MARKETPLACE_READ.operations[p.serviceId.slice('marketplace-'.length) as keyof typeof PUBLIC_MARKETPLACE_READ.operations];
  const service = { provider_id: PUBLIC_DEMO.providerId, service_id: p.serviceId, offer_id: p.offerId, offer_kind: 'compute.execution.v1', resource_kind: 'service', summary: 'Read public catalog data', publication_state: 'active', runtime: 'wasm', package_kind: 'inline_module', entrypoint_kind: 'module', entrypoint: p.entrypoint, contract_version: 'froglet.wasm.host_json.v1', mode: 'sync', module_hash: p.moduleHash, binding_hash: p.bindingHash, capabilities: ['net.http.operation.' + p.operationHash], price_sats: 0, base_fee_msat: 0, success_fee_msat: 0, settlement_method: 'none', starter: '{"provider_id":"' + '0'.repeat(64) + '"}', input_schema: structuredClone(operation.inputSchema), output_schema: structuredClone(operation.outputSchema) };
  return { service, execution_access: 'trial', publication_revision: { revision_hash: p.revisionHash, signer_pubkey: PUBLIC_DEMO.providerId, signature: '3'.repeat(128), payload: { schema_version: 'froglet.publication-revision.v1', provider_id: PUBLIC_DEMO.providerId, service_id: p.serviceId, offer_id: p.offerId, offer_hash: p.offerHash, binding_hash: p.bindingHash, package_digest: p.moduleHash, runtime: 'wasm', package_kind: 'inline_module', build_evidence: { schema_version: 'froglet.publication-build-evidence.v1', builder: 'existing-wasm', builder_version: '0.1.0', source_digest: p.moduleHash, artifact_digest: p.moduleHash, dependency_mode: 'none', hermetic: true }, service: { summary: service.summary, source_kind: 'wasm', starter: service.starter, entrypoint_kind: 'module', entrypoint: p.entrypoint, contract_version: service.contract_version, mode: 'sync', capabilities: service.capabilities, input_schema: service.input_schema, output_schema: service.output_schema }, limits: namedQuote(p).payload.execution_limits, price: { settlement_method: 'none', currency: 'sat', base_amount_minor: 0, success_amount_minor: 0, offer_settlement_method: 'none' }, local_verification: { input_hash: '4'.repeat(64), result_hash: '5'.repeat(64) } } } };
}

test('named metadata is disabled by default; malformed profile configuration preserves the original demo', async () => {
  const upstream = vi.fn(async () => new Response('{"status":"ok"}')); vi.stubGlobal('fetch', upstream);
  const bad = structuredClone(namedProfiles); bad[0].entrypoint = 'operation.wasm';
  const variants = [env(), namedEnv({ FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_ENABLED: 'false' }), namedEnv({ FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_JSON: '{' }), namedEnv({ FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_JSON: JSON.stringify({ providerId: 'a'.repeat(64), profiles: namedProfiles }) }), namedEnv({ FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_JSON: JSON.stringify({ providerId: PUBLIC_DEMO.providerId, profiles: bad }) })];
  for (const settings of variants) {
    const config = await (await worker.fetch(request('/config'), settings as any)).json() as any;
    expect(config.publishedServices).toEqual({ enabled: false, profiles: [] });
    expect((await worker.fetch(request('/v1/provider/services/marketplace-provider'), settings as any)).status).toBe(404);
    expect((await worker.fetch(request('/health'), settings as any)).status).toBe(200);
  }
  expect(upstream.mock.calls.every(call => (call[0] as Request).url.endsWith('/health'))).toBe(true);
});

test('only an explicitly enabled complete public pin set is returned without upstream use', async () => {
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  const config = await (await worker.fetch(request('/config'), namedEnv() as any)).json() as any;
  expect(config.publishedServices).toEqual({ enabled: true, profiles: namedProfiles });
  expect(config.providerId).toBe(PUBLIC_DEMO.providerId); expect(upstream).not.toHaveBeenCalled();
});

test.each(namedIds)('only the fixed named metadata route for %s reaches C7 without credentials', async id => {
  const p = namedProfiles.find(p => p.serviceId === id)!;
  const upstream = vi.fn(async () => new Response(JSON.stringify(namedMetadata(p)), { headers: { 'set-cookie': 'private=secret' } })); vi.stubGlobal('fetch', upstream);
  const response = await worker.fetch(request('/v1/provider/services/' + id, 'GET', undefined, { authorization: 'Bearer private', cookie: 'private=secret' }), namedEnv() as any);
  expect(response.status).toBe(200); expect(upstream).toHaveBeenCalledOnce();
  const forwarded = upstream.mock.calls[0][0] as Request;
  expect(forwarded.url).toBe(PUBLIC_DEMO.origin + '/v1/provider/services/' + id);
  expect(forwarded.headers.has('authorization')).toBe(false); expect(forwarded.headers.has('cookie')).toBe(false);
  expect(response.headers.has('set-cookie')).toBe(false);
});

test.each(namedIds)('named %s quotes forward only the installed module and exact operation grant', async id => {
  const p = namedProfiles.find(p => p.serviceId === id)!;
  const upstream = vi.fn(async () => new Response(JSON.stringify(namedQuote(p)), { status: 201 })); vi.stubGlobal('fetch', upstream);
  const response = await worker.fetch(request('/v1/provider/quotes', 'POST', namedQuoteBody(id)), namedEnv() as any);
  expect(response.status).toBe(201); expect(upstream).toHaveBeenCalledOnce();
  expect(await (upstream.mock.calls[0][0] as Request).json()).toEqual(namedQuoteBody(id));
});

test('valid shaped named retries preserve the exact signed body and key; native remains signature authority', async () => {
  const body = namedDealBody(), upstream = vi.fn(async () => new Response('{"error":"native signature rejected"}', { status: 400 })); vi.stubGlobal('fetch', upstream);
  const response = await worker.fetch(request('/v1/provider/deals', 'POST', body), namedEnv() as any);
  expect(response.status).toBe(400); expect(upstream).toHaveBeenCalledOnce();
  expect(await (upstream.mock.calls[0][0] as Request).json()).toEqual(body);
  body.quote.payload.expires_at = 1; body.deal.payload.admission_deadline = 1;
  // An accepted-but-lost reply may outlive quote expiry. The native idempotency
  // path decides replay; this transport must not invent a new expiry policy.
  await worker.fetch(request('/v1/provider/deals', 'POST', body), namedEnv() as any);
  expect(upstream).toHaveBeenCalledTimes(2);
});

test.each(['module', 'entrypoint', 'capability', 'security', 'kind', 'source', 'mount', 'private-input', 'offer', 'price'])('wrong named %s authority refuses before upstream', async kind => {
  const body: any = namedQuoteBody();
  if (kind === 'module') body.execution.module_hash = 'f'.repeat(64);
  if (kind === 'entrypoint') body.execution.entrypoint.value = 'operation.wasm';
  if (kind === 'capability') body.execution.requested_access.push('net.http.fetch');
  if (kind === 'security') body.execution.security.mode = 'tee';
  if (kind === 'kind') body.execution.workload_kind = 'marketplace.provider';
  if (kind === 'source') body.execution.module_bytes_hex = '0061736d01000000';
  if (kind === 'mount') body.execution.mounts = [{ kind: 'postgres', token: 'private' }];
  if (kind === 'private-input') body.execution.input.url = 'https://internal.invalid';
  if (kind === 'offer') body.offer_id = 'execute.compute.generic';
  if (kind === 'price') body.max_price_sats = 1;
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  expect((await worker.fetch(request('/v1/provider/quotes', 'POST', body), namedEnv() as any)).status).toBe(400);
  expect(upstream).not.toHaveBeenCalled();
});

test.each(['offer', 'descriptor', 'signer', 'requester', 'quote-link', 'workload-link', 'capability', 'runtime', 'memory', 'fuel', 'input', 'output', 'private', 'payment'])('named deal %s field policy refuses before upstream', async kind => {
  const body: any = namedDealBody();
  if (kind === 'offer') body.quote.payload.offer_hash = 'f'.repeat(64);
  if (kind === 'descriptor') body.quote.payload.descriptor_hash = 'f'.repeat(64);
  if (kind === 'signer') body.quote.signer = 'f'.repeat(64);
  if (kind === 'requester') body.deal.payload.requester_id = 'f'.repeat(64);
  if (kind === 'quote-link') body.deal.payload.quote_hash = 'f'.repeat(64);
  if (kind === 'workload-link') body.deal.payload.workload_hash = 'f'.repeat(64);
  if (kind === 'capability') body.quote.payload.capabilities_granted.push('net.http.fetch');
  if (kind === 'runtime') body.quote.payload.execution_limits.max_runtime_ms = 2001;
  if (kind === 'memory') body.quote.payload.execution_limits.max_memory_bytes = 8388609;
  if (kind === 'fuel') body.quote.payload.execution_limits.fuel_limit = 50000001;
  if (kind === 'input') body.quote.payload.execution_limits.max_input_bytes = 2049;
  if (kind === 'output') body.quote.payload.execution_limits.max_output_bytes = 131073;
  if (kind === 'private') body.deal.payload.token = 'private';
  if (kind === 'payment') body.payment = { token: 'private' };
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  expect((await worker.fetch(request('/v1/provider/deals', 'POST', body), namedEnv() as any)).status).toBe(400);
  expect(upstream).not.toHaveBeenCalled();
});

test('a mismatched or private named metadata/quote response is not returned as an approved tool', async () => {
  const wrongMetadata: any = namedMetadata(); wrongMetadata.service.module_bytes_hex = 'private';
  const wrongQuote: any = namedQuote(); wrongQuote.payload.capabilities_granted.push('net.http.fetch');
  for (const [path, body, output, status] of [['/v1/provider/services/marketplace-provider', undefined, wrongMetadata, 200], ['/v1/provider/quotes', namedQuoteBody(), wrongQuote, 201]] as const) {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(output), { status })));
    const response = await worker.fetch(request(path, body ? 'POST' : 'GET', body), namedEnv() as any);
    expect(response.status).toBe(502); expect(await response.text()).not.toContain('private');
  }
});

test('field-policy forwarding never endorses a forged quote as cryptographically verified evidence', async () => {
  const body = namedDealBody(), requester = kernel.newIdentity();
  body.execution.input_hash = kernel.hashJson(body.execution.input);
  body.quote.payload.requester_id = requester.public_key;
  body.quote.payload.workload_hash = kernel.hashJson(body.execution);
  body.deal = kernel.sign(requester.seed_hex, 'deal', body.quote.created_at, {
    ...body.deal.payload, requester_id: requester.public_key, workload_hash: body.quote.payload.workload_hash,
  }) as any;
  expect(verifier.verifyDocument(body.deal).status).toBe('verified');
  expect(verifier.verifyDocument(body.quote).status).toBe('invalid');
  const upstream = vi.fn(async () => new Response('{"error":"native signature rejected"}', { status: 400 })); vi.stubGlobal('fetch', upstream);
  expect((await worker.fetch(request('/v1/provider/deals', 'POST', body), namedEnv() as any)).status).toBe(400);
  expect(upstream).toHaveBeenCalledOnce();
  expect(await (upstream.mock.calls[0][0] as Request).json()).toEqual(body);
});

test('incomplete, duplicate, extra-field and oversized operator pins cannot expose a named service', async () => {
  const duplicate = structuredClone(namedProfiles); duplicate[1] = { ...duplicate[0] } as any;
  const extra = structuredClone(namedProfiles); (extra[0] as any).url = 'https://other.invalid';
  const privateField = { providerId: PUBLIC_DEMO.providerId, profiles: namedProfiles, token: 'private' };
  const variants = [{ providerId: PUBLIC_DEMO.providerId, profiles: namedProfiles.slice(1) }, { providerId: PUBLIC_DEMO.providerId, profiles: duplicate }, { providerId: PUBLIC_DEMO.providerId, profiles: extra }, privateField];
  const upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  for (const value of variants) {
    const settings = namedEnv({ FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_JSON: JSON.stringify(value) });
    const config = await (await worker.fetch(request('/config'), settings as any)).json() as any;
    expect(config.publishedServices).toEqual({ enabled: false, profiles: [] });
    expect((await worker.fetch(request('/v1/provider/services/marketplace-provider'), settings as any)).status).toBe(404);
  }
  const large = namedEnv({ FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_JSON: ' '.repeat(8192) + JSON.stringify({ providerId: PUBLIC_DEMO.providerId, profiles: namedProfiles }) });
  expect((await (await worker.fetch(request('/config'), large as any)).json() as any).publishedServices).toEqual({ enabled: false, profiles: [] });
  expect(upstream).not.toHaveBeenCalled();
});

test.each(['local-fixture', 'build-secret', 'build-component', 'schema-secret', 'starter-secret', 'resource-secret', 'currency-secret'])('named metadata rejects nested %s fields before returning a public reply', async kind => {
  const metadata: any = namedMetadata();
  if (kind === 'local-fixture') metadata.publication_revision.payload.local_verification.private_fixture = { token: 'private-marker' };
  if (kind === 'build-secret') metadata.publication_revision.payload.build_evidence.environment = { token: 'private-marker' };
  if (kind === 'build-component') { metadata.publication_revision.payload.build_evidence.dependency_mode = 'locked'; metadata.publication_revision.payload.build_evidence.components = [{ role: 'package', name: 'package', version: '1', digest: 'a'.repeat(64), token: 'private-marker' }]; }
  if (kind === 'schema-secret') metadata.publication_revision.payload.service.input_schema.properties.private_fixture = { token: 'private-marker' };
  if (kind === 'starter-secret') metadata.service.starter = metadata.publication_revision.payload.service.starter = '{"token":"private-marker"}';
  if (kind === 'resource-secret') metadata.service.resource_kind = { token: 'private-marker' };
  if (kind === 'currency-secret') metadata.service.price_currency = { token: 'private-marker' };
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(metadata))));
  const response = await worker.fetch(request('/v1/provider/services/marketplace-provider'), namedEnv() as any);
  expect(response.status).toBe(502); expect(await response.text()).not.toContain('private-marker');
});

test('the complete transport deadline includes stalled admission and request input without later forwarding', async () => {
  const never = () => new Promise<never>(() => {}), upstream = vi.fn(); vi.stubGlobal('fetch', upstream);
  const cancelled = vi.fn(() => never());
  const body = new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(new TextEncoder().encode('{')); }, cancel: cancelled });
  const stalledInput = new Request('https://froglet.dev/api/public-demo/v1/provider/quotes', { method: 'POST', headers: { 'content-type': 'application/json' }, body, duplex: 'half' } as RequestInit);
  const cases = [[request('/v1/provider/services/marketplace-provider'), namedEnv({ PUBLIC_DEMO_RATE_LIMITER: { limit: never } }), 503], [stalledInput, namedEnv(), 408]] as const;
  const observed = await Promise.all(cases.map(async ([request, settings, expected]) => {
    const started = performance.now();
    let cutoff: ReturnType<typeof setTimeout> | undefined;
    const result = await Promise.race([worker.fetch(request, settings as any).then(response => ({ settled: true, status: response.status })), new Promise<{ settled: false; status: null }>(resolve => { cutoff = setTimeout(() => resolve({ settled: false, status: null }), 15500); })]);
    if (cutoff) clearTimeout(cutoff);
    return { ...result, expected, elapsed: performance.now() - started };
  }));
  expect(observed).toEqual(observed.map(r => ({ ...r, settled: true, status: r.expected })));
  expect(observed.every(r => r.elapsed < 15500)).toBe(true);
  expect(cancelled).toHaveBeenCalledOnce(); expect(upstream).not.toHaveBeenCalled();
}, 20000);

test('the deadline also bounds classic admission and stalled/late native responses without changing normal forwarding', async () => {
  vi.useFakeTimers();
  const never = () => new Promise<never>(() => {}), cancelled = vi.fn(() => never());
  let grantLate: ((v: { success: boolean }) => void) | undefined;
  const lateAdmission = new Promise<{ success: boolean }>(resolve => { grantLate = resolve; });
  const upstream = vi.fn(async (request: Request) => request.url.endsWith('/health')
    ? new Response(new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(new TextEncoder().encode('{')); }, cancel: cancelled }))
    : never());
  vi.stubGlobal('fetch', upstream);
  const classicLate = worker.fetch(request('/demo/status'), env({ PUBLIC_DEMO_RATE_LIMITER: { limit: () => lateAdmission } }) as any);
  const classicBody = worker.fetch(request('/health'), env() as any);
  const namedHeaders = worker.fetch(request('/v1/provider/services/marketplace-provider'), namedEnv() as any);
  await vi.advanceTimersByTimeAsync(15000);
  expect((await classicLate).status).toBe(503); expect((await classicBody).status).toBe(502); expect((await namedHeaders).status).toBe(502);
  grantLate!({ success: true }); await vi.advanceTimersByTimeAsync(1);
  expect(upstream).toHaveBeenCalledTimes(2); expect(cancelled).toHaveBeenCalledOnce();
});
