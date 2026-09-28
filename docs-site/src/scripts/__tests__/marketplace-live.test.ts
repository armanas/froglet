import { afterEach, describe, expect, it, vi } from 'vitest';
vi.mock('../../data/service-link-verifier', () => ({ verifyServiceLinkEvidence: () => ({ valid: true }) }));
import { initMarketplaceLive, serviceAvailability } from '../marketplace-live';
import { getMarketplaceSnapshot } from '../../data/live-snapshot';
import worker from '../../worker';

function page() {
  document.body.innerHTML = `<span data-marketplace-field="refresh">CHECKING</span>
    <main data-marketplace-live><span data-marketplace-field="froglets">—</span>
    <span data-marketplace-field="paidOffers">—</span><span data-marketplace-field="detail"></span>
    <span data-marketplace-field="message"></span>
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
  it('explains a malformed response plainly while retaining diagnostics', async () => {
    vi.useFakeTimers(); page();
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('<!DOCTYPE html>Unavailable')));
    initMarketplaceLive();
    await vi.advanceTimersByTimeAsync(0);
    expect(document.querySelector('[data-marketplace-field="message"]')?.textContent).toBe('We could not load the catalog. We will try again automatically.');
    expect(document.querySelector('[data-marketplace-field="detail"]')?.textContent).toContain('JSON');
    expect(document.querySelector('[data-marketplace-field="refresh"]')?.textContent).toBe('UNAVAILABLE');
  });
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
    expect(document.querySelector('[data-marketplace-field="refresh"]')?.textContent).toBe('CATALOG UPDATED');
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
  it('shows a public HTTPS endpoint instead of a provider loopback endpoint', async () => {
    vi.stubGlobal('fetch', vi.fn(async (url: string) => new Response(JSON.stringify(
      url.includes('healthz') ? { status: 'ok' } : url.includes('providers') ? { items: [{
        provider_id: 'p', descriptor: { transport_endpoints: [
          { transport: 'http', uri: 'http://127.0.0.1:28080' },
          { transport: 'https', uri: 'https://provider.relay.froglet.dev' },
        ] },
      }] } : url.includes('offers') ? { items: [] } : {}
    ), { status: url.includes('deals') ? 404 : 200 })));
    const result = await getMarketplaceSnapshot();
    expect(result.providers[0].endpoint).toBe('https://provider.relay.froglet.dev');
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

describe('service discovery cards', () => {
  function cardsPage() {
    document.body.innerHTML = `<span data-marketplace-field="refresh"></span><main data-marketplace-live>
      <div data-marketplace-search-form><input data-marketplace-search /><output data-marketplace-search-count></output></div>
      <select data-marketplace-filter><option value="all">All</option><option value="ready">Ready</option><option value="free">Free</option></select>
      <div data-marketplace-service-cards></div><p data-marketplace-no-results hidden>No matches</p></main>`;
  }
  it('labels invitation listings as metadata-only even when the reachability lease is healthy', async () => {
    vi.useFakeTimers(); cardsPage();
    await refresh(snapshot({ offers: [{
      offerId:'invited-catalog', providerId:'ab'.repeat(32), settlementMethod:'none', baseFeeMsat:0, successFeeMsat:0,
      availability:{status:'healthy', admission:'invitation_required', lastCheckedAt:Date.now()/1000, leaseExpiresAt:Date.now()/1000+60},
    }] }));
    expect(document.querySelector('.service-card')?.textContent).toContain('Invitation required');
    expect(document.querySelector('.service-card')?.textContent).toContain('execution has not been tested');
    expect(document.querySelector('.service-availability')?.textContent).toBe('Recently checked');
  });
  it('requires an unexpired check and never labels stale data recently checked', () => {
    const offer = { availability: { status:'healthy', leaseExpiresAt:200, lastCheckedAt:100 } } as any;
    expect(serviceAvailability(offer, false, 150_000).ready).toBe(true);
    expect(serviceAvailability(offer, false, 200_000).label).toBe('Check expired');
    expect(serviceAvailability(offer, true, 150_000).ready).toBe(false);
    expect(serviceAvailability({} as any).ready).toBe(false);
  });
  it('does not advertise incomplete pricing as free or include it in the free filter', async () => {
    vi.useFakeTimers(); cardsPage();
    await refresh(snapshot({offers:[{offerId:'unknown-price', providerId:'ab'.repeat(32), pricingKnown:false, settlementMethod:'none', baseFeeMsat:0, successFeeMsat:0}]}));
    expect(document.querySelector('.service-card-top')?.textContent).toContain('Price unavailable');
    const filter = document.querySelector('select')!;
    filter.value='free'; filter.dispatchEvent(new Event('change'));
    expect(document.querySelector<HTMLElement>('.service-card')?.hidden).toBe(true);
  });
  it('renders safe readable links, supports multiword searches and filters, and expires checks', async () => {
    vi.useFakeTimers(); cardsPage();
    const provider = 'ab'.repeat(32);
    await refresh(snapshot({offers:[{
      offerId:'hla-catalog', providerId:provider, offerKind:'catalog', runtime:'builtin', settlementMethod:'none', baseFeeMsat:0, successFeeMsat:0,
      sharePath:`/s/${provider}/hla-catalog`, summary:'Skin peptide catalog <script>bad()</script>',
      availability:{status:'healthy', lastCheckedAt:Date.now()/1000, leaseExpiresAt:Date.now()/1000+20},
    }]}));
    expect(document.querySelector('.service-card h3')?.textContent).toBe('HLA Catalog');
    expect(document.querySelector('.service-card script')).toBeNull();
    expect(document.querySelector('.service-open')?.getAttribute('href')).toBe(`/s/${provider}/hla-catalog`);
    const input = document.querySelector('input')!;
    input.value = 'skin peptide'; input.dispatchEvent(new Event('input'));
    expect(document.querySelector<HTMLElement>('.service-card')?.hidden).toBe(false);
    const space = new KeyboardEvent('keydown', {key:' ',bubbles:true,cancelable:true});
    input.dispatchEvent(space); expect(space.defaultPrevented).toBe(false);
    const filter = document.querySelector('select')!;
    filter.value = 'ready'; filter.dispatchEvent(new Event('change'));
    expect(document.querySelector('[data-marketplace-search-count]')?.textContent).toBe('1 service');
    await vi.advanceTimersByTimeAsync(30_000);
    expect(document.querySelector<HTMLElement>('.service-card')?.hidden).toBe(true);
    expect(document.querySelector<HTMLElement>('[data-marketplace-no-results]')?.hidden).toBe(false);
  });
});
