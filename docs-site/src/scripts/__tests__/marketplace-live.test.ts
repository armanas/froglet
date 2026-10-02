import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it, vi } from 'vitest';
vi.mock('../../data/service-link-verifier', () => ({ verifyServiceLinkEvidence: () => ({ valid: true }) }));
import { initMarketplaceLive, serviceAvailability } from '../marketplace-live';
import { getMarketplaceSnapshot } from '../../data/live-snapshot';
import worker from '../../worker';

// The dashboard's real markup, so these tests fail if the page and the script drift apart.
const pageSource = readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), '../../pages/marketplace.astro'), 'utf8');
const dashboardMarkup = pageSource.slice(pageSource.indexOf('<div class="mkt"'), pageSource.indexOf('</StoryLayout>'));
const provider = 'ab'.repeat(32);

function page() {
  document.body.innerHTML = `<span data-marketplace-field="refresh">CHECKING</span>
    <main data-marketplace-live><span data-marketplace-field="froglets">—</span>
    <span data-marketplace-field="paidOffers">—</span><span data-marketplace-field="detail"></span>
    <span data-marketplace-field="message"></span>
    <table><tbody data-marketplace-provider-table></tbody></table></main>`;
}
function dashboard() { document.body.innerHTML = dashboardMarkup; }

const snapshot = (overrides = {}) => ({
  checkedAt: new Date().toISOString(), status: 'pass', detail: 'Read API available',
  providerCount: 0, offerCount: 100, providers: [],
  offers: [{ offerId: 'free', providerId: 'provider', runtime: 'wasm', settlementMethod: 'none', baseFeeMsat: 0, successFeeMsat: 0 }],
  dealFeed: { status: 'pending', detail: 'Not available', deals: [] }, ...overrides,
});
const catalog = (offers: unknown[], overrides = {}) => snapshot({ offers, offerCount: offers.length, ...overrides });
const listing = (overrides = {}) => ({
  offerId: 'sample', providerId: provider, offerKind: 'catalog', runtime: 'builtin', packageKind: 'builtin',
  settlementMethod: 'none', baseFeeMsat: 0, successFeeMsat: 0, artifactHash: 'c'.repeat(64), ...overrides,
});
const checked = (leaseSeconds = 60) => ({ status: 'healthy', admission: 'open', lastCheckedAt: Date.now() / 1000, leaseExpiresAt: Date.now() / 1000 + leaseSeconds });
const shared = (id: string, overrides = {}) => listing({ offerId: id, serviceId: id, sharePath: `/s/${provider}/${id}`, availability: checked(), ...overrides });
const priced = (id: string, overrides = {}) => listing({ offerId: id, settlementMethod: 'lightning', baseFeeMsat: 3000, successFeeMsat: 5000, ...overrides });

const $ = <T extends HTMLElement = HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
const field = (name: string) => $(`[data-marketplace-field="${name}"]`).textContent;
const items = () => Array.from(document.querySelectorAll<HTMLElement>('tbody.mkt-item'));
const nameOf = (item: HTMLElement) => item.querySelector('.mkt-name')!.textContent;
const names = () => items().map(nameOf);
const shown = () => items().filter((item) => !item.hidden).map(nameOf);
const count = () => $('[data-marketplace-search-count]').textContent;
const category = (value: string) => $<HTMLInputElement>(`[data-marketplace-categories] input[value="${value}"]`);
const toggle = (name: 'ready' | 'free') => $<HTMLInputElement>(`[data-marketplace-filter="${name}"]`);
const type = (input: HTMLInputElement, value: string) => { input.value = value; input.dispatchEvent(new Event('input')); };

async function refresh(body: unknown, status = 200) {
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(body), { status })));
  initMarketplaceLive();
  await vi.advanceTimersByTimeAsync(0);
}
async function respond(body: unknown, status = 200) {
  vi.mocked(fetch).mockImplementation(async () => new Response(JSON.stringify(body), { status }));
  $('[data-marketplace-refresh]').click();
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

describe('the live dashboard header', () => {
  it('starts as a loading skeleton whose spans match the table header', () => {
    dashboard();
    const columns = $<HTMLTableElement>('[data-marketplace-service-list]').tHead!.rows[0].cells.length;
    const skeleton = Array.from(document.querySelectorAll<HTMLTableCellElement>('.mkt-skeleton-rows td'));
    expect(skeleton.length).toBeGreaterThan(0);
    expect(skeleton.every((cell) => cell.colSpan === columns)).toBe(true);
    expect(count()).toBe('Loading');
  });

  it('shows LIVE with the totals, sample sizes and a plain-language message', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(snapshot({
      providerCount: 5, offerCount: 30,
      providers: [{ providerId: provider, descriptorHash: 'd'.repeat(64), serviceKinds: ['compute'], executionRuntimes: [], endpoint: 'https://node.example.dev', successCount: 9, failureCount: 1, totalSettledMsat: 12_000, lastReceiptFinishedAt: 1 }],
      offers: [listing({ offerId: 'a' }), priced('b'), listing({ offerId: 'c', pricingKnown: false })],
    }));
    expect($('[data-marketplace-field="refresh"]').textContent).toBe('LIVE');
    expect($('[data-marketplace-field="refresh"]').dataset.status).toBe('live');
    expect(field('message')).toBe('Catalog updated. Open a service to see what it does.');
    expect(field('froglets')).toBe('5');
    expect(field('offers')).toBe('30');
    expect(field('freeOffers')).toBe('1');
    expect(field('paidOffers')).toBe('1');
    expect(field('receipts')).toBe('10');
    expect(field('successRate')).toBe('90%');
    expect(field('settledSats')).toBe('12sats');
    expect(field('scope')).toBe('Showing 3 of 30 indexed offers. Search and filters cover only the offers loaded here.');
  });

  it('says how many indexed offers are shown when the sample is the whole catalog', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([listing()]));
    expect(field('scope')).toBe('Showing all 1 indexed offer.');
  });

  it('counts down to the next check and keeps every age fresh each second', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([shared('hla-catalog')]));
    await vi.advanceTimersByTimeAsync(10_000);
    expect($('[data-marketplace-age]').textContent).toBe('Updated 10s ago');
    expect($('[data-marketplace-next]').textContent).toBe('Next check in 20s');
    expect($('time.mkt-age').textContent).toBe('checked 10s ago');
  });

  it('fills the provider and runtime panels and copies the top provider', async () => {
    vi.useFakeTimers(); dashboard();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    await refresh(catalog([listing({ runtime: 'wasm' }), listing({ offerId: 'two', runtime: 'wasm' }), listing({ offerId: 'three', runtime: 'builtin' })], {
      providerCount: 1,
      providers: [{ providerId: provider, descriptorHash: 'd'.repeat(64), serviceKinds: ['compute'], executionRuntimes: [], endpoint: 'https://node.example.dev', successCount: 2, failureCount: 0, totalSettledMsat: 0, lastReceiptFinishedAt: 1 }],
    }));
    expect($('[data-marketplace-provider-table]').textContent).toContain('node.example.dev');
    expect($('[data-marketplace-services-breakdown]').textContent).toContain('wasm');
    expect(document.querySelectorAll('[data-marketplace-offer-book] tr')).toHaveLength(3);
    $('[data-marketplace-copy-provider]').click();
    await vi.advanceTimersByTimeAsync(0);
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining(`provider_id: ${provider}`));
  });

  it('copies evidence that says what the catalog does not prove', async () => {
    vi.useFakeTimers(); dashboard();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    await refresh(catalog([listing()]));
    const button = $('[data-marketplace-copy-summary]');
    button.click();
    await vi.advanceTimersByTimeAsync(0);
    const evidence = writeText.mock.calls[0][0] as string;
    expect(evidence).toContain('Froglet marketplace evidence');
    expect(evidence).toContain('indexed_receipt_value:');
    expect(evidence).toContain('not_proved: hosted paid rails');
    expect(button.textContent).toBe('Copied');
  });
});

describe('service listing', () => {
  it('draws one row per offer with an icon, title, type, price, availability and byline', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([
      listing({ offerId: 'events.query', offerKind: 'events.query' }),
      shared('hla-catalog', { summary: 'Skin peptides' }),
    ]));
    expect(document.querySelector('.mkt-skeleton-rows')).toBeNull();
    expect($('[data-marketplace-service-list]').getAttribute('aria-busy')).toBe('false');
    expect(names()).toEqual(['HLA Catalog', 'Read node events']);

    const [service, events] = items();
    expect(service.querySelector<HTMLElement>('.mkt-icon')!.dataset.kind).toBe('service');
    expect(service.querySelector('.mkt-tag')!.textContent).toBe('Shared services');
    expect(service.querySelector('.service-price')!.textContent).toBe('Free');
    expect(service.querySelector('.service-price')!.classList.contains('is-free')).toBe(true);
    expect(service.querySelector('.service-availability')!.textContent).toBe('Recently checked');
    expect(service.querySelector<HTMLElement>('.service-availability')!.dataset.state).toBe('ready');
    expect(service.querySelector('.mkt-sub')!.textContent).toBe('Skin peptides');
    expect(service.querySelector('.mkt-id')!.textContent).toMatch(/^hla-catalog · /);
    expect(service.querySelector<HTMLElement>('.mkt-id')!.title).toBe(provider);
    expect(events.querySelector('.mkt-tag')!.textContent).toBe('Events');
    expect(events.querySelector('.service-availability')!.textContent).toBe('Availability not confirmed');
  });

  it('keeps every row and its details drawer as wide as the table header', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([shared('hla-catalog'), listing()]));
    const columns = $<HTMLTableElement>('[data-marketplace-service-list]').tHead!.rows[0].cells.length;
    for (const item of items()) {
      expect(item.querySelector('.mkt-row')!.children).toHaveLength(columns);
      expect(item.querySelector<HTMLTableCellElement>('.mkt-detail td')!.colSpan).toBe(columns);
    }
  });

  it('offers Open only for a well-formed share path', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([
      shared('hla-catalog'),
      listing({ offerId: 'script', sharePath: 'javascript:alert(1)' }),
      listing({ offerId: 'offsite', sharePath: '//evil.example/x' }),
      listing({ offerId: 'traversal', sharePath: `/s/${provider}/../x` }),
    ]));
    const links = Array.from(document.querySelectorAll('.service-open'));
    expect(links).toHaveLength(1);
    expect(links[0].getAttribute('href')).toBe(`/s/${provider}/hla-catalog`);
    expect(links[0].getAttribute('aria-label')).toBe('Open HLA Catalog');
    expect(field('sharedServices')).toBe('1');
    expect(Array.from(document.querySelectorAll('.mkt-detail a')).every((a) => !/javascript:|evil\.example|\.\./.test(a.getAttribute('href') ?? ''))).toBe(true);
  });

  it('renders provider-supplied text as text, never as markup', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([shared('hla-catalog', { summary: 'Skin <script>bad()</script><img src=x onerror=bad()>' })]));
    expect(document.querySelector('.mkt-item script, .mkt-item img')).toBeNull();
    expect($('.mkt-sub').textContent).toBe('Skin <script>bad()</script><img src=x onerror=bad()>');
  });

  it('labels invitation listings as metadata-only even when the reachability lease is healthy', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([listing({ offerId: 'invited-catalog', availability: { ...checked(), admission: 'invitation_required' } })]));
    expect($('.mkt-item').textContent).toContain('Invitation required');
    expect($('.mkt-item').textContent).toContain('execution has not been tested');
    expect($('.service-availability').textContent).toBe('Recently checked');
  });

  it('does not advertise incomplete pricing as free or include it in the free filter', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([listing({ offerId: 'unknown-price', pricingKnown: false })]));
    expect($('.service-price').textContent).toBe('Price unavailable');
    expect($('.service-price').classList.contains('is-free')).toBe(false);
    expect(items()[0].dataset.free).toBe('false');
    toggle('free').click();
    expect(items()[0].hidden).toBe(true);
  });

  it('explains a priced offer as base plus success fee, and only a priced offer', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([priced('paid-lookup'), listing({ offerId: 'free-lookup' })]));
    const [free, paid] = items();
    expect(paid.querySelector('.service-price')!.textContent).toBe('3 + 5 sats');
    expect(paid.querySelector('.mkt-price-note')!.textContent).toBe('base + success fee');
    expect(paid.querySelector('.mkt-note--price')!.textContent).toBe('Price is base + success fee; inspect payment terms before use.');
    expect(free.querySelector('.mkt-price-note, .mkt-note--price')).toBeNull();
  });

  it('keeps the readable title, byline and description of the marketplace’s own offers', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([listing({ offerId: 'marketplace.search', offerKind: 'marketplace.search' })]));
    expect(nameOf(items()[0])).toBe('Search the marketplace');
    expect($('.mkt-id').textContent).toMatch(/^marketplace\.search · /);
    expect($('.mkt-sub').textContent).toBe('Search the marketplace for providers and offers.');
    expect($('.mkt-tag').textContent).toBe('Marketplace');
  });
});

describe('browsing, searching and filtering', () => {
  const mixed = () => catalog([
    shared('hla-catalog', { summary: 'Skin peptide catalog' }),
    listing({ offerId: 'events.query', offerKind: 'events.query' }),
    priced('execute.compute', { offerKind: 'compute.wasm.v1' }),
  ]);

  it('lists the categories that have offers, with counts, and always lists shared services', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(mixed());
    const tally = (value: string) => $(`[data-marketplace-category-count="${value}"]`).textContent;
    expect([tally('all'), tally('service'), tally('events'), tally('compute')]).toEqual(['3', '1', '1', '1']);
    expect(category('marketplace')).toBeNull();
    expect(category('other')).toBeNull();
  });

  it('keeps Shared services in the sidebar at zero so an empty state is discoverable', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([listing({ offerId: 'events.query', offerKind: 'events.query' })]));
    expect($('[data-marketplace-category-count="service"]').textContent).toBe('0');
    expect(category('other')).toBeNull();
  });

  it('narrows by category, recently-checked and free, and counts what it shows', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(mixed());
    expect(count()).toBe('3 services');

    category('events').click();
    expect(shown()).toEqual(['Read node events']);
    expect(count()).toBe('1 service');

    category('all').click();
    toggle('ready').click();
    expect(shown()).toEqual(['HLA Catalog']);

    toggle('ready').click();
    toggle('free').click();
    expect(shown().sort()).toEqual(['HLA Catalog', 'Read node events']);
    expect(count()).toBe('2 services');
  });

  it('keeps the chosen category when the catalog refreshes', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(mixed());
    category('compute').click();
    await respond(mixed());
    expect(category('compute').checked).toBe(true);
    expect(shown()).toEqual(['Run a WebAssembly job']);
  });

  it('falls back to all services when the chosen category disappears', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(mixed());
    category('compute').click();
    await respond(catalog([shared('hla-catalog')]));
    expect(category('all').checked).toBe(true);
    expect(shown()).toEqual(['HLA Catalog']);
  });

  it('searches across name, id, description and provider with several words, and leaves the space key alone', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(mixed());
    const input = $<HTMLInputElement>('[data-marketplace-search]');
    type(input, 'skin peptide');
    expect(shown()).toEqual(['HLA Catalog']);
    type(input, 'hla-catalog');
    expect(shown()).toEqual(['HLA Catalog']);
    type(input, 'EVENTS query');
    expect(shown()).toEqual(['Read node events']);
    type(input, provider.slice(0, 12));
    expect(shown()).toHaveLength(3);
    const space = new KeyboardEvent('keydown', { key: ' ', bubbles: true, cancelable: true });
    input.dispatchEvent(space);
    expect(space.defaultPrevented).toBe(false);
  });

  it('shows a way back when nothing matches, and the reset button restores everything', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(mixed());
    const input = $<HTMLInputElement>('[data-marketplace-search]');
    toggle('free').click();
    type(input, 'zzz-no-such-service');
    expect(shown()).toEqual([]);
    expect(count()).toBe('0 services');
    expect($('[data-marketplace-no-results]').hidden).toBe(false);

    $('[data-marketplace-reset]').click();
    expect(input.value).toBe('');
    expect(toggle('free').checked).toBe(false);
    expect(category('all').checked).toBe(true);
    expect(shown()).toHaveLength(3);
    expect($('[data-marketplace-no-results]').hidden).toBe(true);
    expect(document.activeElement).toBe(input);
  });

  it('focuses the search box with "/" but not while typing, and clears it with Escape', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(mixed());
    const input = $<HTMLInputElement>('[data-marketplace-search]');
    const slash = new KeyboardEvent('keydown', { key: '/', bubbles: true, cancelable: true });
    document.body.dispatchEvent(slash);
    expect(slash.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(input);

    const typed = new KeyboardEvent('keydown', { key: '/', bubbles: true, cancelable: true });
    input.dispatchEvent(typed);
    expect(typed.defaultPrevented).toBe(false);

    type(input, 'events');
    expect(shown()).toEqual(['Read node events']);
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(input.value).toBe('');
    expect(shown()).toHaveLength(3);
  });

  it('stops calling a listing recently checked once its lease expires', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([shared('hla-catalog', { availability: checked(20) })]));
    toggle('ready').click();
    expect(count()).toBe('1 service');
    await vi.advanceTimersByTimeAsync(30_000);
    expect($('.service-availability').textContent).toBe('Check expired');
    expect($('[data-marketplace-field="refresh"]').textContent).toBe('LIVE');
    expect(shown()).toEqual([]);
    expect(count()).toBe('0 services');
    expect($('[data-marketplace-no-results]').hidden).toBe(false);
    expect(field('recentServices')).toBe('0');
  });
});

describe('freshness and failure', () => {
  it('never labels stale data recently checked', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([shared('hla-catalog')], { checkedAt: new Date(Date.now() - 120_000).toISOString() }));
    expect($('[data-marketplace-field="refresh"]').textContent).toBe('STALE');
    expect($('.service-availability').textContent).toBe('Status needs refresh');
    expect(field('recentServices')).toBe('0');
    expect(field('message')).toBe('Showing an earlier catalog. Availability may have changed; we are checking again.');
  });

  it('turns a live listing stale when a refresh fails, and live again when the next one works', async () => {
    vi.useFakeTimers(); dashboard();
    const body = () => catalog([shared('hla-catalog')]);
    await refresh(body());
    expect($('.service-availability').textContent).toBe('Recently checked');
    expect(field('recentServices')).toBe('1');

    vi.mocked(fetch).mockRejectedValue(new Error('network offline'));
    await vi.advanceTimersByTimeAsync(30_000);
    expect($('[data-marketplace-field="refresh"]').textContent).toBe('STALE');
    expect($('.service-availability').textContent).toBe('Status needs refresh');
    expect(field('recentServices')).toBe('0');
    expect(names()).toEqual(['HLA Catalog']);

    await respond(body());
    expect($('[data-marketplace-field="refresh"]').textContent).toBe('LIVE');
    expect($('.service-availability').textContent).toBe('Recently checked');
    expect(field('recentServices')).toBe('1');
  });

  it('shows an error with a retry when the first load fails, and recovers on retry', async () => {
    vi.useFakeTimers(); dashboard();
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    initMarketplaceLive();
    await vi.advanceTimersByTimeAsync(0);

    const banner = $('[data-marketplace-banner]');
    expect(banner.hidden).toBe(false);
    expect(banner.dataset.mode).toBe('error');
    expect(banner.getAttribute('role')).toBe('alert');
    expect(count()).toBe('Unavailable');
    expect($('[data-marketplace-field="refresh"]').textContent).toBe('UNAVAILABLE');
    expect(field('froglets')).toBe('—');
    expect(field('scope')).toBe('The catalog could not be loaded.');
    expect(document.querySelector('.mkt-skeleton-rows')).toBeNull();

    vi.mocked(fetch).mockImplementation(async () => new Response(JSON.stringify(catalog([shared('hla-catalog')]))));
    $('[data-marketplace-retry]').click();
    await vi.advanceTimersByTimeAsync(0);
    expect(banner.hidden).toBe(true);
    expect(names()).toEqual(['HLA Catalog']);
    expect($('[data-marketplace-field="refresh"]').textContent).toBe('LIVE');
  });

  it('invites people to share a service when nothing is shared, and steps aside when something is', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([listing({ offerId: 'events.query', offerKind: 'events.query' })]));
    const banner = $('[data-marketplace-banner]');
    expect(banner.hidden).toBe(false);
    expect(banner.dataset.mode).toBe('empty');
    expect(banner.getAttribute('role')).toBe('status');
    expect(banner.textContent).toContain('No shared services are listed right now');
    expect(Array.from(banner.querySelectorAll('a')).map((a) => a.getAttribute('href'))).toEqual(['/publish/', '/learn/share-services/']);

    category('service').click();
    expect(banner.hidden).toBe(false);
    expect($('[data-marketplace-no-results]').hidden).toBe(true);
    expect(count()).toBe('0 services');

    category('events').click();
    expect(banner.hidden).toBe(true);

    category('all').click();
    await respond(catalog([shared('hla-catalog')]));
    expect(banner.hidden).toBe(true);
  });

  it('does not report "no services match" for an empty catalog', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([]));
    expect(count()).toBe('0 services');
    expect($('[data-marketplace-no-results]').hidden).toBe(true);
    expect($('[data-marketplace-banner]').hidden).toBe(false);
  });
});

describe('sorting, details and change highlights', () => {
  const trio = () => catalog([
    listing({ offerId: 'gamma-lookup', pricingKnown: false }),
    priced('beta-lookup'),
    listing({ offerId: 'alpha-lookup' }),
  ]);
  const header = (key: string) => $(`[data-sort="${key}"]`);
  const sortState = () => Array.from(document.querySelectorAll('thead th[aria-sort]')).map((th) => `${th.querySelector('[data-sort]')!.getAttribute('data-sort')}:${th.getAttribute('aria-sort')}`);

  it('starts with the most available listings first and reverses on the first click of Status', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([listing({ offerId: 'a-offline', availability: { status: 'offline', lastCheckedAt: 1, leaseExpiresAt: 1 } }), shared('b-ready'), listing({ offerId: 'c-unknown' })]));
    expect(names()).toEqual(['B Ready', 'C Unknown', 'A Offline']);
    expect(sortState()).toEqual(['status:ascending']);
    header('status').click();
    expect(names()).toEqual(['A Offline', 'C Unknown', 'B Ready']);
    expect(sortState()).toEqual(['status:descending']);
  });

  it('sorts by price with free first, then reverses, and unknown prices last', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(trio());
    header('price').click();
    expect(names()).toEqual(['Alpha Lookup', 'Beta Lookup', 'Gamma Lookup']);
    expect(sortState()).toEqual(['price:ascending']);
    header('price').click();
    expect(names()).toEqual(['Gamma Lookup', 'Beta Lookup', 'Alpha Lookup']);
    expect(sortState()).toEqual(['price:descending']);
    header('price').click();
    expect(sortState()).toEqual(['price:ascending']);
  });

  it('sorts by name and by type, and keeps the order after a refresh', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(trio());
    header('name').click();
    expect(names()).toEqual(['Alpha Lookup', 'Beta Lookup', 'Gamma Lookup']);
    await respond(trio());
    expect(names()).toEqual(['Alpha Lookup', 'Beta Lookup', 'Gamma Lookup']);
    header('category').click();
    expect(sortState()).toEqual(['category:ascending']);
  });

  it('opens and closes a details drawer with the exact identifiers', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([priced('beta-lookup', { artifactHash: 'e'.repeat(64) })]));
    const button = $('[data-marketplace-toggle]');
    const drawer = $(`#${button.getAttribute('aria-controls')}`);
    expect(button.getAttribute('aria-expanded')).toBe('false');
    expect(drawer.hidden).toBe(true);

    button.click();
    expect(button.getAttribute('aria-expanded')).toBe('true');
    expect(drawer.hidden).toBe(false);
    expect(items()[0].classList.contains('is-open')).toBe(true);
    expect(drawer.textContent).toContain('beta-lookup');
    expect(drawer.textContent).toContain('Base 3000 msat · success 5000 msat');
    expect(drawer.textContent).toContain('e'.repeat(64));
    expect(drawer.textContent).toContain(provider);

    button.click();
    expect(button.getAttribute('aria-expanded')).toBe('false');
    expect(drawer.hidden).toBe(true);
    expect(items()[0].classList.contains('is-open')).toBe(false);
  });

  it('copies a value from the drawer and says so, and says when copying failed', async () => {
    vi.useFakeTimers(); dashboard();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    Object.assign(document, { execCommand: vi.fn(() => false) });
    await refresh(catalog([listing({ offerId: 'beta-lookup' })]));
    const button = $('[data-copy-value]');
    expect(button.dataset.copyValue).toBe('beta-lookup');
    button.click();
    await vi.advanceTimersByTimeAsync(0);
    expect(writeText).toHaveBeenCalledWith('beta-lookup');
    expect(button.textContent).toBe('Copied');
    await vi.advanceTimersByTimeAsync(1600);
    expect(button.textContent).toBe('Copy');

    writeText.mockRejectedValue(new Error('denied'));
    button.click();
    await vi.advanceTimersByTimeAsync(0);
    expect(button.textContent).toBe('Copy failed');
  });

  it('highlights only what changed since the last check, and clears the highlight when it ends', async () => {
    vi.useFakeTimers(); dashboard();
    await refresh(catalog([priced('beta-lookup'), listing({ offerId: 'alpha-lookup' })]));
    expect(document.querySelectorAll('.is-new, .is-updated')).toHaveLength(0);

    await respond(catalog([priced('beta-lookup', { baseFeeMsat: 9000 }), listing({ offerId: 'alpha-lookup' }), listing({ offerId: 'delta-lookup' })]));
    const row = (name: string) => items().find((item) => nameOf(item) === name)!;
    expect(row('Beta Lookup').classList.contains('is-updated')).toBe(true);
    expect(row('Delta Lookup').classList.contains('is-new')).toBe(true);
    expect(row('Alpha Lookup').classList.contains('is-updated') || row('Alpha Lookup').classList.contains('is-new')).toBe(false);

    row('Beta Lookup').dispatchEvent(new Event('animationend'));
    expect(row('Beta Lookup').classList.contains('is-updated')).toBe(false);
  });
});

describe('service availability', () => {
  it('requires an unexpired check and never labels stale data recently checked', () => {
    const offer = { availability: { status: 'healthy', leaseExpiresAt: 200, lastCheckedAt: 100 } } as any;
    expect(serviceAvailability(offer, false, 150_000).ready).toBe(true);
    expect(serviceAvailability(offer, false, 200_000).label).toBe('Check expired');
    expect(serviceAvailability(offer, true, 150_000).ready).toBe(false);
    expect(serviceAvailability({} as any).ready).toBe(false);
  });
});
