import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../data/service-link-verifier', () => ({ verifyServiceLinkEvidence: () => ({ valid: true }) }));

import worker from '../../worker';

const provider = '11'.repeat(32);
const service = 'catalog';
const hash = (byte: string) => byte.repeat(64);
const offerHash = hash('a');
const descriptorHash = hash('b');
const bindingHash = hash('c');
const fixture = [
  { service: { provider_id: provider, service_id: service, offer_id: 'offer', binding_hash: bindingHash }, publication_revision: { revision_hash: hash('d'), payload: { provider_id: provider, service_id: service, offer_id: 'offer', binding_hash: bindingHash, offer_hash: offerHash, runtime: 'builtin', service: { summary: 'Public catalog for agents', starter: '{"op":"describe"}', input_schema: { type: 'object', properties: { op: { type: 'string' } } }, output_schema: { type: 'object' } }, limits: { max_input_bytes: 4096 }, price: { settlement_method: 'none', currency: 'sat', base_amount_minor: 0, success_amount_minor: 0, offer_settlement_method: 'none' } } } },
  { hash: offerHash, payload: { provider_id: provider, offer_id: 'offer', descriptor_hash: descriptorHash, settlement_method: 'none', price_schedule: { base_fee_msat: 0, success_fee_msat: 0 } } },
  { hash: descriptorHash, payload: { provider_id: provider } },
];
const env = { ASSETS: { fetch: vi.fn(async () => new Response('legacy page')) } };

function readyFetch() {
  let index = 0;
  vi.stubGlobal('fetch', vi.fn(async () => {
    const value = fixture[index++] as any;
    if (value?.hash && value?.payload) {
      const kind = value.payload.descriptor_hash ? 'offer' : 'descriptor';
      return new Response(JSON.stringify({ kind, hash: value.hash, document: { ...value, artifact_type: kind } }));
    }
    return new Response(JSON.stringify(value));
  }));
}

afterEach(() => { vi.unstubAllGlobals(); env.ASSETS.fetch.mockClear(); });

describe('Worker service-link routes', () => {
  it('serves service-specific initial HTML, machine JSON, Markdown, and HEAD', async () => {
    const root = `https://froglet.dev/s/${provider}/${service}`;
    readyFetch();
    const page = await worker.fetch(new Request(root), env);
    const html = await page.text();
    expect(page.status).toBe(200);
    expect(page.headers.get('content-type')).toContain('text/html');
    expect(html).toContain('<h1>Public catalog for agents</h1>');
    expect(html).toContain(`${root}/manifest.json`);
    expect(html).toContain('Price</dt><dd>Free');
    expect(page.headers.get('link')).toContain('agent.md');

    readyFetch();
    const manifest = await worker.fetch(new Request(`${root}/manifest.json`), env);
    expect(manifest.headers.get('content-type')).toContain('application/json');
    const body = await manifest.json() as any;
    expect(body.presentation.title).toBe('Public catalog for agents');
    expect(body.contract.price.kind).toBe('free');
    expect(body.evidence.verification_state).toBe('verified');

    readyFetch();
    const markdown = await worker.fetch(new Request(`${root}/agent.md`), env);
    expect(markdown.headers.get('content-type')).toContain('text/markdown');
    expect(await markdown.text()).toContain('Public catalog for agents');

    readyFetch();
    const head = await worker.fetch(new Request(root, { method: 'HEAD' }), env);
    expect(head.status).toBe(200);
    expect(await head.text()).toBe('');
  });

  it('keeps malformed references and unsupported historical revisions out of the resolver', async () => {
    const fetcher = vi.fn(); vi.stubGlobal('fetch', fetcher);
    expect((await worker.fetch(new Request('https://froglet.dev/s/not-a-provider/catalog'), env)).status).toBe(400);
    expect((await worker.fetch(new Request(`https://froglet.dev/s/${provider}/${service}?revision=${hash('d')}`), env)).status).toBe(400);
    expect((await worker.fetch(new Request(`https://froglet.dev/s/${provider}/${service}`, { method: 'POST' }), env)).status).toBe(405);
    expect(fetcher).not.toHaveBeenCalled();
  });

  it('returns an explicit unknown state when the provider cannot be reached', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    const response = await worker.fetch(new Request(`https://froglet.dev/s/${provider}/${service}/manifest.json`), env);
    expect(response.status).toBe(503);
    const body = await response.json() as any;
    expect(body.availability.state).toBe('unknown');
    expect(body.contract).toBeNull();
  });

  it('keeps a verified description understandable during an outage without claiming it is callable', async () => {
    const values = new Map<string, string>();
    const cache = { get: vi.fn(async (key: string) => values.get(key) ?? null), put: vi.fn(async (key: string, value: string) => { values.set(key, value); }) };
    const cachedEnv = { ...env, SERVICE_LINK_CACHE: cache };
    const url = `https://froglet.dev/s/${provider}/${service}`;
    readyFetch();
    expect((await worker.fetch(new Request(url), cachedEnv)).status).toBe(200);
    expect(cache.put).toHaveBeenCalledOnce();
    readyFetch();
    expect((await worker.fetch(new Request(url), cachedEnv)).status).toBe(200);
    expect(cache.put).toHaveBeenCalledOnce();
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    const response = await worker.fetch(new Request(url), cachedEnv);
    const page = await response.text();
    expect(response.status).toBe(200);
    expect(page).toContain('Public catalog for agents');
    expect(page).toContain('The provider could not be reached now');
    expect(page).toContain('do not call based on this page alone');
    expect(cache.get).toHaveBeenCalledTimes(3);
  });

  it('redirects released publisher links to the agent-readable page', async () => {
    const response = await worker.fetch(new Request(`https://froglet.dev/service/?provider=${provider}&service=${service}`), env);
    expect(response.status).toBe(302);
    expect(response.headers.get('location')).toBe(`https://froglet.dev/s/${provider}/${service}`);
    expect(env.ASSETS.fetch).not.toHaveBeenCalled();
    expect((await worker.fetch(new Request(`https://froglet.dev/service/?provider=${provider}&provider=${provider}&service=${service}`), env)).status).toBe(400);
    expect((await worker.fetch(new Request('https://froglet.dev/service/?provider=bad&service=catalog'), env)).status).toBe(400);
  });

  it('keeps the legacy landing page when there is no service reference', async () => {
    const response = await worker.fetch(new Request('https://froglet.dev/service/'), env);
    expect(await response.text()).toBe('legacy page');
    expect(env.ASSETS.fetch).toHaveBeenCalledOnce();
  });
});

describe('portable link sharing', () => {
  it('negotiates plain text and JSON on the canonical URL without affecting browser HTML', async () => {
    const url = `https://froglet.dev/s/${provider}/${service}`;
    for (const [accept, type] of [['text/plain','text/plain'], ['application/json','application/json'], ['text/html,application/xhtml+xml,*/*','text/html']]) {
      readyFetch();
      const response = await worker.fetch(new Request(url, {headers:{accept}}), env);
      expect(response.headers.get('content-type')).toContain(type);
      expect(response.headers.get('vary')).toBe('Accept');
      expect(response.headers.get('access-control-allow-origin')).toBe('*');
    }
  });
  it('provides QR downloads without contacting the provider, even when it is offline', async () => {
    const fetcher = vi.fn(); vi.stubGlobal('fetch', fetcher);
    const response = await worker.fetch(new Request(`https://froglet.dev/s/${provider}/${service}/qr.svg?download=1`), env);
    expect(response.status).toBe(200);
    expect(response.headers.get('content-type')).toContain('image/svg+xml');
    expect(response.headers.get('content-disposition')).toBe('attachment; filename="catalog-qr.svg"');
    expect(await response.text()).toContain('<svg');
    expect(fetcher).not.toHaveBeenCalled();
  });
});
