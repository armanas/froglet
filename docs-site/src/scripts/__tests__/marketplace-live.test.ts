import { afterEach, describe, expect, it, vi } from 'vitest';
import { initMarketplaceLive } from '../marketplace-live';
import { getMarketplaceSnapshot } from '../../data/live-snapshot';
import worker from '../../worker';

function page() {
  document.body.innerHTML = `<span data-marketplace-field="refresh">CHECKING</span>
    <main data-marketplace-live><span data-marketplace-field="froglets">—</span>
    <span data-marketplace-field="paidOffers">—</span><span data-marketplace-field="detail"></span>
    <table><tbody data-marketplace-provider-table></tbody></table></main>`;
}
const snapshot = (overrides = {}) => ({
  checkedAt: new Date().toISOString(), status: 'pass', detail: 'Read API available',
  providerCount: 0, offerCount: 100, providers: [],
  offers: [{ offerId: 'free', providerId: 'provider', runtime: 'wasm', settlementMethod: 'none', baseFeeMsat: 0, successFeeMsat: 0 }],
  dealFeed: { status: 'pending', detail: 'Not available', deals: [] }, ...overrides,
});
async function refresh(body: unknown, status = 200) {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify(body), { status })));
  initMarketplaceLive();
  await vi.advanceTimersByTimeAsync(0);
}
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); document.body.innerHTML = ''; });

describe('marketplace runtime status', () => {
  it('reports an upstream HTTP failure without leaking a JSON parser error', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('error code: 1016', { status: 530 })));
    const result = await getMarketplaceSnapshot();
    expect(result.status).toBe('fail');
    expect(result.detail).toBe('Marketplace is unavailable (HTTP 530). Retrying automatically.');
    expect(result.dealFeed.detail).toContain('A public deal feed is not available');
  });
  it('does not call an HTTP-successful failed snapshot live or show zero activity', async () => {
    vi.useFakeTimers(); page();
    await refresh(snapshot({ status: 'fail', detail: 'Upstream unavailable' }));
    expect(document.querySelector('[data-marketplace-field="refresh"]')?.textContent).toBe('UNAVAILABLE');
    expect(document.querySelector('[data-marketplace-field="froglets"]')?.textContent).toBe('—');
    expect(document.querySelector('[data-marketplace-provider-table]')?.textContent).toContain('unavailable');
  });
  it('labels old data stale and calculates sampled priced offers from sampled offers', async () => {
    vi.useFakeTimers(); page();
    await refresh(snapshot({ checkedAt: new Date(Date.now() - 120_000).toISOString() }));
    expect(document.querySelector('[data-marketplace-field="refresh"]')?.textContent).toBe('STALE');
    expect(document.querySelector('[data-marketplace-field="paidOffers"]')?.textContent).toBe('0');
  });
  it('marks a successful snapshot stale when the next request fails', async () => {
    vi.useFakeTimers(); page(); await refresh(snapshot({ providerCount: 3 }));
    expect(document.querySelector('[data-marketplace-field="refresh"]')?.textContent).toBe('LIVE');
    vi.mocked(fetch).mockRejectedValue(new Error('network offline'));
    await vi.advanceTimersByTimeAsync(30_000);
    expect(document.querySelector('[data-marketplace-field="refresh"]')?.textContent).toBe('STALE');
    expect(document.querySelector('[data-marketplace-field="froglets"]')?.textContent).toBe('3');
  });
  it('rejects malformed upstream rows and keeps complete artifact hashes', async () => {
    const hash = 'a'.repeat(64);
    vi.stubGlobal('fetch', vi.fn(async (url: string) => new Response(JSON.stringify(
      url.includes('healthz') ? { status: 'ok' } : url.includes('providers') ? { items: [{ provider_id: 'p', current_descriptor_hash: hash }] }
      : url.includes('offers') ? { items: [{ artifact_hash: hash }] } : {}
    ), { status: url.includes('deals') ? 404 : 200 })));
    const result = await getMarketplaceSnapshot();
    expect(result.providers[0].descriptorHash).toBe(hash);
    expect(result.offers[0].artifactHash).toBe(hash);
    vi.mocked(fetch).mockResolvedValue(new Response(JSON.stringify({ status: 'ok' })));
    expect((await getMarketplaceSnapshot()).status).toBe('fail');
  });
  it('serves snapshot errors at runtime and never delegates the API to static assets', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    const assets = { fetch: vi.fn() };
    const response = await worker.fetch(new Request('https://froglet.dev/api/marketplace-snapshot'), { ASSETS: assets });
    expect(response.status).toBe(502);
    expect(response.headers.get('cache-control')).toBe('no-store');
    expect(assets.fetch).not.toHaveBeenCalled();
  });
});
