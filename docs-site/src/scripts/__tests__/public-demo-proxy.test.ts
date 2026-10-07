// @vitest-environment node
import { afterEach, expect, test, vi } from 'vitest';
// The unrelated service-link route imports Worker-specific Wasm. This test exercises HTTP routing only.
vi.mock('../../data/service-link-verifier', () => ({ verifyServiceLinkEvidence: () => ({ valid: true }) }));
import worker from '../../worker';
import { PUBLIC_DEMO } from '../../data/public-demo-config';

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
