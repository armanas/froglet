// @vitest-environment node
import { afterEach, expect, test, vi } from 'vitest';
// The unrelated service-link route imports Worker-specific Wasm. This test exercises HTTP routing only.
vi.mock('../../data/service-link-verifier', () => ({ verifyServiceLinkEvidence: () => ({ valid: true }) }));
import worker from '../../worker';
import { PUBLIC_DEMO } from '../../data/public-demo-config';

afterEach(() => vi.unstubAllGlobals());

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
